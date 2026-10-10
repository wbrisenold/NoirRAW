use super::*;
use anyhow::{Context, Result};
use image::DynamicImage;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{error, info, warn};

// Model URLs (InsightFace models)
const SCRFD_MODEL_URL_HF: &str = "https://huggingface.co/ykk648/face_lib/resolve/main/face_detect/scrfd_onnx/scrfd_500m_bnkps.onnx";
const SCRFD_MODEL_URL_GH: &str = "https://github.com/deepinsight/insightface/releases/download/v0.7/scrfd_500m_bnkps.onnx";
const ARCFACE_MODEL_URL_PRIMARY: &str = "https://huggingface.co/maze/faceX/resolve/e010b5098c3685fd00b22dd2aec6f37320e3d850/w600k_r50.onnx";

pub const FACE_CLUSTER_BATCH_SIZE: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceBbox {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct FaceEmbedding {
    pub embedding: Vec<f32>,
    pub bbox: FaceBbox,
    pub asset_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Face {
    pub id: i64,
    pub image_id: i64,
    pub bbox: [f32; 4], // x, y, w, h (normalized 0-1)
    pub landmarks: Option<[[f32; 2]; 5]>,
    pub embedding: Option<Vec<f32>>,
    pub person_id: Option<i64>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub id: i64,
    pub name: String,
    pub face_count: i32,
    pub avatar_face_id: Option<i64>,
    pub created_time: i64,
    pub updated_time: i64,
}

type ScrfdPreprocessResult = ([i64; 4], Vec<f32>, f32, f32, f32);

pub struct FaceProcessor {
    models_dir: PathBuf,
    use_gpu: bool,
    scrfd_session: Option<Mutex<ort::session::Session>>,
    arcface_session: Option<Mutex<ort::session::Session>>,
}

impl FaceProcessor {
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            use_gpu: std::env::var("NOIRRAW_FACE_USE_GPU")
                .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE"))
                .unwrap_or(false),
            scrfd_session: None,
            arcface_session: None,
        }
    }

    pub fn scrfd_loaded(&self) -> bool { self.scrfd_session.is_some() }
    pub fn arcface_loaded(&self) -> bool { self.arcface_session.is_some() }

    pub async fn initialize(&mut self) -> Result<()> {
        std::fs::create_dir_all(&self.models_dir)
            .context("Failed to create models directory")?;

        let auto_dl = std::env::var("NOIRRAW_FACE_AUTO_DOWNLOAD")
            .map(|v| !matches!(v.as_str(), "0" | "false" | "FALSE"))
            .unwrap_or(true);
        
        if auto_dl {
            if let Err(e) = self.download_models().await {
                warn!("Face model auto-download failed: {}", e);
            }
        } else {
            info!("Face model auto-download disabled by user.");
        }

        if let Err(e) = self.load_models().await {
            warn!("Face models not loaded: {}", e);
        }
        Ok(())
    }

    async fn download_models(&self) -> Result<()> {
        let scrfd_path = self.models_dir.join("scrfd_500m_bnkps.onnx");
        let arcface_path = self.models_dir.join("w600k_r50.onnx");
        let client = self.create_http_client()?;

        if !scrfd_path.exists() {
            info!("Downloading SCRFD face detection model...");
            if let Err(e) = self.download_file(&client, SCRFD_MODEL_URL_HF, &scrfd_path).await {
                warn!("Failed to download from Hugging Face: {}. Trying GitHub...", e);
                self.download_file(&client, SCRFD_MODEL_URL_GH, &scrfd_path).await?;
            }
        }

        if !arcface_path.exists() {
            info!("Downloading ArcFace recognition model (w600k_r50.onnx)...");
            self.download_file(&client, ARCFACE_MODEL_URL_PRIMARY, &arcface_path).await?;
        }

        Ok(())
    }

    fn create_http_client(&self) -> Result<reqwest::Client> {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(token) = std::env::var("HF_TOKEN") {
            if !token.is_empty() {
                info!("Using Hugging Face token for model download.");
                headers.insert(
                    reqwest::header::AUTHORIZATION,
                    reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))?
                );
            }
        }
        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .context("Failed to create HTTP client")
    }

    async fn download_file(&self, client: &reqwest::Client, url: &str, path: &Path) -> Result<()> {
        let response = client
            .get(url)
            .send()
            .await
            .context(format!("Failed to download model from {}", url))?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to download model: HTTP {}", response.status());
        }
        let bytes = response.bytes().await.context("Failed to read response body")?;
        
        std::fs::write(path, &bytes).context(format!("Failed to write file: {:?}", path))?;
        
        let metadata = std::fs::metadata(path)
            .context(format!("Failed to read metadata for downloaded file: {:?}", path))?;
        if metadata.len() != bytes.len() as u64 {
            anyhow::bail!(
                "File integrity check failed: expected {} bytes, got {} bytes",
                bytes.len(),
                metadata.len()
            );
        }
        
        if bytes.len() < 1024 {
            anyhow::bail!("Downloaded file is suspiciously small ({} bytes), may be corrupted", bytes.len());
        }
        
        info!("Downloaded model to {:?} ({} bytes, verified)", path, bytes.len());
        Ok(())
    }

    async fn load_models(&mut self) -> Result<()> {
        let scrfd_path = self.models_dir.join("scrfd_500m_bnkps.onnx");
        let arcface_path = self.models_dir.join("w600k_r50.onnx");

        if !scrfd_path.exists() || !arcface_path.exists() {
            anyhow::bail!(
                "Face models missing; expected SCRFD at {:?} and ArcFace at {:?}",
                scrfd_path, arcface_path
            );
        }

        let scrfd_builder = ort::session::Session::builder()?;
        let arc_builder = ort::session::Session::builder()?;

        if self.use_gpu {
            info!("GPU support for face models requires ort CUDA feature - using CPU for now");
        } else {
            info!("Using CPU for face models");
        }

        let scrfd = scrfd_builder
            .commit_from_file(&scrfd_path)
            .context("Failed to create SCRFD session")?;
        let arc = arc_builder
            .commit_from_file(&arcface_path)
            .context("Failed to create ArcFace session")?;

        self.scrfd_session = Some(Mutex::new(scrfd));
        self.arcface_session = Some(Mutex::new(arc));
        info!("Face models loaded: SCRFD={:?} ArcFace={:?}", scrfd_path, arcface_path);
        Ok(())
    }

    fn preprocess_scrfd(&self, image: &DynamicImage) -> Result<ScrfdPreprocessResult> {
        let (ow, oh) = (image.width() as f32, image.height() as f32);
        let scale = 640.0 / ow.max(oh);
        let nw = (ow * scale) as u32;
        let nh = (oh * scale) as u32;
        let resized = image.resize_exact(nw, nh, image::imageops::FilterType::Triangle);
        let mut padded = image::DynamicImage::new_rgb8(640, 640);
        image::imageops::overlay(&mut padded, &resized, 0, 0);
        let rgb = padded.to_rgb8();
        let mut data = Vec::with_capacity(3 * 640 * 640);
        for c in 0..3 {
            for y in 0..640u32 {
                for x in 0..640u32 {
                    let p = rgb.get_pixel(x, y);
                    let v = match c { 0 => p[2], 1 => p[1], _ => p[0] } as f32;
                    data.push((v - 127.5) / 128.0);
                }
            }
        }
        Ok(([1, 3, 640, 640], data, scale, ow, oh))
    }

    fn preprocess_arcface(&self, face_crop: &DynamicImage) -> Result<([i64; 4], Vec<f32>)> {
        let resized = face_crop.resize_exact(112, 112, image::imageops::FilterType::Triangle);
        let rgb = resized.to_rgb8();
        let mut data = Vec::with_capacity(3 * 112 * 112);
        for c in 0..3 {
            for y in 0..112u32 {
                for x in 0..112u32 {
                    let p = rgb.get_pixel(x, y);
                    let v = match c { 0 => p[0], 1 => p[1], _ => p[2] } as f32;
                    data.push((v - 127.5) / 128.0);
                }
            }
        }
        Ok(([1, 3, 112, 112], data))
    }

    pub fn detect_faces(&self, image: &DynamicImage) -> Result<Vec<FaceBbox>> {
        let session_guard = self
            .scrfd_session
            .as_ref()
            .context("Detection model not loaded")?
            .lock();
        let (shape, data, scale, _orig_w, _orig_h) = self.preprocess_scrfd(image)?;
        
        let img_w = image.width() as f32;
        let img_h = image.height() as f32;

        let input_name = session_guard.inputs[0].name.clone();
        let input = ort::value::Value::from_array((shape.to_vec(), data))
            .context("Failed to create SCRFD input tensor")?;
        let outputs = session_guard
            .run(ort::inputs![input_name => input])
            .context("SCRFD inference failed")?;

        let mut all_scores = Vec::new();
        let mut all_boxes = Vec::new();
        
        for scale_str in ["8", "16", "32"] {
            if let (Some(sv), Some(bv)) = (outputs.get(format!("score_{}", scale_str).as_str()), outputs.get(format!("bbox_{}", scale_str).as_str())) {
                if let (Ok((_, scores)), Ok((_, boxes))) = (sv.try_extract_tensor::<f32>(), bv.try_extract_tensor::<f32>()) {
                    all_scores.extend_from_slice(scores);
                    all_boxes.extend_from_slice(boxes);
                }
            }
        }
        
        if all_scores.is_empty() {
            let score_val = outputs.get("score_8").or_else(|| outputs.get("score"));
            let bbox_val = outputs.get("bbox_8").or_else(|| outputs.get("bbox"));
            
            if let (Some(sv), Some(bv)) = (score_val, bbox_val) {
                if let (Ok((_, scores)), Ok((_, boxes))) = (sv.try_extract_tensor::<f32>(), bv.try_extract_tensor::<f32>()) {
                    all_scores.extend_from_slice(scores);
                    all_boxes.extend_from_slice(boxes);
                }
            }
        }
        
        if all_scores.is_empty() {
            return Ok(vec![]);
        }

        let base_confidence_threshold: f32 = std::env::var("NOIRRAW_FACE_CONFIDENCE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.20);
        
        let mut raw: Vec<FaceBbox> = Vec::new();
        
        for stride_str in ["8", "16", "32"] {
            let stride: f32 = stride_str.parse().unwrap();
            let score_tensor_opt = outputs.get(format!("score_{}", stride_str).as_str())
                .or_else(|| if stride == 8.0 { outputs.get("score") } else { None });
            let bbox_tensor_opt = outputs.get(format!("bbox_{}", stride_str).as_str())
                .or_else(|| if stride == 8.0 { outputs.get("bbox") } else { None });
            
            let (scores, boxes, height, width) = if let (Some(sv), Some(bv)) = (score_tensor_opt, bbox_tensor_opt) {
                if let (Ok((s_shape, s_data)), Ok((b_shape, b_data))) = (sv.try_extract_tensor::<f32>(), bv.try_extract_tensor::<f32>()) {
                    let h = (640.0 / stride) as usize;
                    let w = (640.0 / stride) as usize;
                    (s_data, b_data, h, w)
                } else {
                    continue;
                }
            } else {
                continue;
            };

            let num_grid_points = height * width;
            if scores.len() % num_grid_points != 0 {
                continue;
            }
            let anchors_per_point = scores.len() / num_grid_points;

            for i in 0..num_grid_points {
                let cy = (i / width) as f32 * stride;
                let cx = (i % width) as f32 * stride;
                
                for a in 0..anchors_per_point {
                    let idx = i * anchors_per_point + a;
                    let conf = scores[idx];
                    if conf < base_confidence_threshold { continue; }
                    
                    let b = idx * 4;
                    let l = boxes[b] * stride;
                    let t = boxes[b+1] * stride;
                    let r = boxes[b+2] * stride;
                    let bb = boxes[b+3] * stride;
                    
                    let x1_640 = cx - l;
                    let y1_640 = cy - t;
                    let x2_640 = cx + r;
                    let y2_640 = cy + bb;
                    
                    let x1 = (x1_640 / scale).max(0.0).min(img_w);
                    let y1 = (y1_640 / scale).max(0.0).min(img_h);
                    let x2 = (x2_640 / scale).max(0.0).min(img_w);
                    let y2 = (y2_640 / scale).max(0.0).min(img_h);
                    
                    if x2 <= x1 || y2 <= y1 { continue; }
                    let w_px = x2 - x1;
                    let h_px = y2 - y1;
                    if w_px < 8.0 || h_px < 8.0 { continue; }
                    
                    raw.push(FaceBbox { x1, y1, x2, y2, confidence: conf });
                }
            }
        }
        
        let nms_iou_threshold: f32 = std::env::var("NOIRRAW_FACE_NMS_IOU_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.4);
        let keep = nms_wrapper(&raw, nms_iou_threshold);
        let mut out = Vec::with_capacity(keep.len());
        for idx in keep { out.push(raw[idx].clone()); }
        Ok(out)
    }

    pub fn recognize_face(&self, face_crop: &DynamicImage) -> Result<Vec<f32>> {
        let session_guard = self
            .arcface_session
            .as_ref()
            .context("Recognition model not loaded")?
            .lock();
        let (shape, data) = self.preprocess_arcface(face_crop)?;

        let input_name = session_guard.inputs[0].name.clone();
        let input = ort::value::Value::from_array((shape.to_vec(), data))
            .context("Failed to create ArcFace input tensor")?;
        let outputs = session_guard
            .run(ort::inputs![input_name => input])
            .context("ArcFace inference failed")?;
        
        let mut output_key: Option<&str> = None;
        for k in outputs.keys() {
            if k == "output" || k == "embedding" || k == "fc1" || k == "features" {
                output_key = Some(k);
                break;
            }
        }
        
        if output_key.is_none() {
            if let Some(first_key) = outputs.keys().next() {
                output_key = Some(first_key);
            }
        }
        
        if let Some(key) = output_key {
            if let Some(val) = outputs.get(key) {
                if let Ok((_, slice)) = val.try_extract_tensor::<f32>() {
                    let mut v = slice.to_vec();
                    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                    if norm > 0.0 {
                        for x in &mut v { *x /= norm; }
                        return Ok(v);
                    }
                }
            }
        }
        Ok(vec![])
    }

    pub fn process_image(&self, asset_id: i64, image_path: &Path) -> Result<Vec<FaceEmbedding>> {
        let ext = image_path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        
        let allowed = ["jpg", "jpeg", "png", "webp", "heic", "heif", "tiff", "tif", "arw", "cr2", "cr3", "nef", "raf", "rw2", "orf", "pef", "srw", "dng"];
        if !allowed.contains(&ext.as_str()) {
            return Ok(vec![]);
        }
        
        let img = image::open(image_path).context(format!("Failed to open image: {:?}", image_path))?;
        let bboxes = self.detect_faces(&img)?;
        if bboxes.is_empty() {
            return Ok(vec![]);
        }
        let mut embeddings = Vec::new();
        for bbox in bboxes {
            let x1 = bbox.x1.max(0.0) as u32;
            let y1 = bbox.y1.max(0.0) as u32;
            let x2 = bbox.x2.min(img.width() as f32) as u32;
            let y2 = bbox.y2.min(img.height() as f32) as u32;
            if x2 > x1 && y2 > y1 {
                let face_crop = img.crop_imm(x1, y1, x2 - x1, y2 - y1);
                match self.recognize_face(&face_crop) {
                    Ok(embedding) => {
                        embeddings.push(FaceEmbedding {
                            embedding,
                            bbox: bbox.clone(),
                            asset_id,
                        });
                    }
                    Err(e) => warn!("Failed to generate embedding for face: {}", e),
                }
            }
        }
        Ok(embeddings)
    }
}

pub fn cluster_faces_hdbscan(
    embeddings: &[FaceEmbedding],
    min_cluster_size: usize,
    min_samples: usize,
) -> Vec<Vec<usize>> {
    if embeddings.is_empty() {
        return vec![];
    }

    let epsilon: f32 = std::env::var("NOIRRAW_FACE_CLUSTER_EPSILON")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.55);

    let mut visited = vec![false; embeddings.len()];
    let mut clusters = Vec::new();

    for i in 0..embeddings.len() {
        if visited[i] { continue; }
        visited[i] = true;
        let mut neighbors = find_neighbors_dbscan(embeddings, i, epsilon);
        if neighbors.len() < min_samples { continue; }

        let mut cluster = vec![i];
        let mut j = 0;
        while j < neighbors.len() {
            let neighbor_idx = neighbors[j];
            if !visited[neighbor_idx] {
                visited[neighbor_idx] = true;
                let neighbor_neighbors = find_neighbors_dbscan(embeddings, neighbor_idx, epsilon);
                if neighbor_neighbors.len() >= min_samples {
                    neighbors.extend(neighbor_neighbors);
                }
            }
            if !cluster.contains(&neighbor_idx) {
                cluster.push(neighbor_idx);
            }
            j += 1;
        }

        if cluster.len() >= min_cluster_size {
            clusters.push(cluster);
        }
    }

    clusters
}

fn find_neighbors_dbscan(embeddings: &[FaceEmbedding], idx: usize, epsilon: f32) -> Vec<usize> {
    let embedding = &embeddings[idx].embedding;
    let mut neighbors = Vec::new();
    for (i, other) in embeddings.iter().enumerate() {
        if i == idx { continue; }
        let distance = cosine_distance(embedding, &other.embedding);
        if distance <= epsilon { neighbors.push(i); }
    }
    neighbors
}

pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() { return 1.0; }
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { return 1.0; }
    1.0 - (dot_product / (norm_a * norm_b))
}

fn nms_wrapper(boxes: &[FaceBbox], iou_threshold: f32) -> Vec<usize> {
    if boxes.is_empty() { return vec![]; }
    let mut indices: Vec<usize> = (0..boxes.len()).collect();
    indices.sort_by(|&a, &b| {
        boxes[b].confidence.partial_cmp(&boxes[a].confidence).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut keep = Vec::new();
    let mut suppressed = vec![false; boxes.len()];
    for i in 0..indices.len() {
        let ia = indices[i];
        if suppressed[ia] { continue; }
        keep.push(ia);
        for &ib in indices.iter().skip(i + 1) {
            if suppressed[ib] { continue; }
            if calculate_iou(&boxes[ia], &boxes[ib]) > iou_threshold {
                suppressed[ib] = true;
            }
        }
    }
    keep
}

fn calculate_iou(a: &FaceBbox, b: &FaceBbox) -> f32 {
    let x1 = a.x1.max(b.x1);
    let y1 = a.y1.max(b.y1);
    let x2 = a.x2.min(b.x2);
    let y2 = a.y2.min(b.y2);
    if x2 <= x1 || y2 <= y1 { return 0.0; }
    let intersection = (x2 - x1) * (y2 - y1);
    let area_a = (a.x2 - a.x1) * (a.y2 - a.y1);
    let area_b = (b.x2 - b.x1) * (b.y2 - b.y1);
    let union = area_a + area_b - intersection;
    if union <= 0.0 { return 0.0; }
    intersection / union
}

pub struct FaceIndex {
    id_to_index: HashMap<i64, usize>,
    index_to_id: Vec<i64>,
    embeddings: Vec<Vec<f32>>,
}

impl Default for FaceIndex {
    fn default() -> Self { Self::new() }
}

impl FaceIndex {
    pub fn new() -> Self {
        Self {
            id_to_index: HashMap::new(),
            index_to_id: Vec::new(),
            embeddings: Vec::new(),
        }
    }

    pub fn add_embedding(&mut self, face_id: i64, embedding: &[f32]) {
        let index_pos = self.index_to_id.len();
        self.id_to_index.insert(face_id, index_pos);
        self.index_to_id.push(face_id);
        self.embeddings.push(embedding.to_vec());
    }

    pub fn find_similar(&self, embedding: &[f32], k: usize, threshold: f32) -> Vec<(i64, f32)> {
        let mut results: Vec<(i64, f32)> = self
            .embeddings
            .iter()
            .enumerate()
            .map(|(idx, emb)| {
                let dist = cosine_distance(embedding, emb);
                let face_id = self.index_to_id.get(idx).copied().unwrap_or(-1);
                (face_id, dist)
            })
            .filter(|(_, dist)| *dist <= threshold)
            .collect();

        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        results
    }

    pub fn clear(&mut self) {
        self.id_to_index.clear();
        self.index_to_id.clear();
        self.embeddings.clear();
    }
}