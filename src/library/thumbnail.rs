use super::*;
use anyhow::Result;
use image::ImageFormat;
use std::path::Path;

pub struct ThumbnailGenerator {
    max_dimension: u32,
    quality: u8,
}

impl Default for ThumbnailGenerator {
    fn default() -> Self {
        Self {
            max_dimension: 360,
            quality: 85,
        }
    }
}

impl ThumbnailGenerator {
    pub fn new(max_dimension: u32, quality: u8) -> Self {
        Self { max_dimension, quality }
    }

    pub fn generate(&self, path: &Path) -> Result<Vec<u8>> {
        let img = image::open(path)?;
        let thumb = img.thumbnail(self.max_dimension, self.max_dimension);
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, self.quality);
        encoder.encode_image(&thumb)?;
        Ok(bytes)
    }

    pub fn generate_from_bytes(&self, data: &[u8]) -> Result<Vec<u8>> {
        let img = image::load_from_memory(data)?;
        let thumb = img.thumbnail(self.max_dimension, self.max_dimension);
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, self.quality);
        encoder.encode_image(&thumb)?;
        Ok(bytes)
    }

    pub fn generate_from_scene_image(&self, scene: &crate::image_io::SceneImage) -> Result<Vec<u8>> {
        let width = scene.width;
        let height = scene.height;
        let pixels: Vec<u8> = scene
            .pixels
            .iter()
            .flat_map(|p| {
                let r = (p[0].clamp(0.0, 1.0) * 255.0) as u8;
                let g = (p[1].clamp(0.0, 1.0) * 255.0) as u8;
                let b = (p[2].clamp(0.0, 1.0) * 255.0) as u8;
                [r, g, b]
            })
            .collect();

        let img = image::RgbImage::from_raw(width, height, pixels)
            .ok_or_else(|| anyhow::anyhow!("Failed to create image from scene"))?;
        let thumb = image::imageops::thumbnail(&img, self.max_dimension, self.max_dimension);
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, self.quality);
        encoder.encode_image(&thumb)?;
        Ok(bytes)
    }
}

// Slint-compatible thumbnail cache
use slint::Image;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

pub struct ThumbnailCache {
    cache: Arc<Mutex<HashMap<String, Image>>>,
    generator: ThumbnailGenerator,
}

impl ThumbnailCache {
    pub fn new(generator: ThumbnailGenerator) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            generator,
        }
    }

    pub fn get_or_generate(&self, path: &Path) -> Option<Image> {
        let key = path.to_string_lossy().to_string();
        
        // Check cache first
        if let Ok(cache) = self.cache.lock() {
            if let Some(img) = cache.get(&key) {
                return Some(img.clone());
            }
        }

        // Generate thumbnail
        if let Ok(bytes) = self.generator.generate(path) {
            if let Ok(img) = Image::load_from_memory(&bytes) {
                if let Ok(mut cache) = self.cache.lock() {
                    cache.insert(key.clone(), img.clone());
                }
                return Some(img);
            }
        }
        None
    }

    pub fn get_or_generate_from_bytes(&self, key: &str, data: &[u8]) -> Option<Image> {
        if let Ok(cache) = self.cache.lock() {
            if let Some(img) = cache.get(key) {
                return Some(img.clone());
            }
        }

        if let Ok(bytes) = self.generator.generate_from_bytes(data) {
            if let Ok(img) = Image::load_from_memory(&bytes) {
                if let Ok(mut cache) = self.cache.lock() {
                    cache.insert(key.to_string(), img.clone());
                }
                return Some(img);
            }
        }
        None
    }

    pub fn clear(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }
}