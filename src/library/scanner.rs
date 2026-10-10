use super::*;
use crate::image_io::SceneImage;
use anyhow::{Context, Result};
use chrono::Utc;
use image::ImageFormat;
use quick_xml::{Reader, events::{Event, BytesStart, BytesEnd, BytesDecl, BytesText}};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[cfg(feature = "exif")]
use kamadak_exif::{Exif, Tag};

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub recursive: bool,
    pub follow_symlinks: bool,
    pub extract_embedded_preview: bool,
    pub generate_thumbnails: bool,
    pub thumbnail_max_dimension: u32,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            recursive: true,
            follow_symlinks: false,
            extract_embedded_preview: true,
            generate_thumbnails: true,
            thumbnail_max_dimension: 360,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScannedImage {
    pub path: PathBuf,
    pub filename: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub modified_time: i64,
    pub imported_time: i64,
    pub rating: i16,
    pub color_label: i16,
    pub pick_reject: i16,
    pub has_raw: bool,
    pub mime_type: String,
    pub exif_data: Option<ExifData>,
    pub embedded_preview: Option<Vec<u8>>, // JPEG bytes
    pub thumbnail: Option<Vec<u8>>,        // JPEG bytes
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExifData {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<String>,
    pub aperture: Option<String>,
    pub shutter_speed: Option<String>,
    pub iso: Option<String>,
    pub date_taken: Option<String>,
    pub exposure_compensation: Option<String>,
    pub white_balance: Option<String>,
    pub orientation: Option<u16>,
    pub gps: Option<GpsData>,
    pub color_space: Option<String>,
    pub exposure_program: Option<String>,
    pub metering_mode: Option<String>,
    pub flash: Option<String>,
    pub focal_length_35mm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpsData {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
}

pub struct LibraryScanner {
    db: LibraryDb,
    options: ScanOptions,
}

impl LibraryScanner {
    pub fn new(db: LibraryDb, options: ScanOptions) -> Self {
        Self { db, options }
    }

    pub fn scan_folder(&self, folder: &Path) -> Result<ScanStats> {
        let start = std::time::Instant::now();
        let mut stats = ScanStats::default();

        let walker = WalkDir::new(folder)
            .follow_links(self.options.follow_symlinks)
            .max_depth(if self.options.recursive { usize::MAX } else { 1 });

        let entries: Vec<_> = walker
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .collect();

        stats.total_files = entries.len();

        let results: Vec<_> = entries
            .par_iter()
            .filter_map(|entry| {
                let path = entry.path();
                if !Self::is_supported_extension(path) {
                    return None;
                }
                match self.process_file(path) {
                    Ok(scanned) => Some(Ok(scanned)),
                    Err(e) => {
                        log::warn!("Failed to process {}: {}", path.display(), e);
                        Some(Err(e))
                    }
                }
            })
            .collect();

        for result in results {
            match result {
                Ok(scanned) => {
                    if let Err(e) = self.db.insert_image(&scanned.into()) {
                        log::error!("Failed to insert image into DB: {}", e);
                        stats.errors += 1;
                    } else {
                        stats.imported += 1;
                        if scanned.embedded_preview.is_some() {
                            stats.with_embedded_preview += 1;
                        }
                        if scanned.thumbnail.is_some() {
                            stats.with_thumbnails += 1;
                        }
                    }
                }
                Err(_) => stats.errors += 1,
            }
        }

        stats.duration_ms = start.elapsed().as_millis() as u64;
        Ok(stats)
    }

    fn is_supported_extension(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| super::SUPPORTED_EXTENSIONS.iter().any(|&ext| ext.eq_ignore_ascii_case(e)))
            .unwrap_or(false)
    }

    fn process_file(&self, path: &Path) -> Result<ScannedImage> {
        let metadata = fs::metadata(path)?;
        let file_size = metadata.len();
        let modified_time = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or_else(|| Utc::now().timestamp());

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let sha256 = Self::compute_sha256(path)?;
        let mime_type = Self::guess_mime_type(path);

        let (width, height, exif_data, embedded_preview, thumbnail, has_raw) =
            Self::extract_image_info(path, mime_type.as_str(), &self.options)?;

        Ok(ScannedImage {
            path: path.to_path_buf(),
            filename,
            sha256,
            width,
            height,
            file_size,
            modified_time,
            imported_time: Utc::now().timestamp(),
            rating: 0,
            color_label: 0,
            pick_reject: 0,
            has_raw,
            mime_type,
            exif_data,
            embedded_preview,
            thumbnail,
        })
    }

    fn compute_sha256(path: &Path) -> Result<String> {
        let mut hasher = Sha256::new();
        let mut file = fs::File::open(path)?;
        std::io::copy(&mut file, &mut hasher)?;
        Ok(hex::encode(hasher.finalize()))
    }

    fn guess_mime_type(path: &Path) -> String {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        match ext.as_str() {
            "arw" => "image/x-sony-arw",
            "cr2" => "image/x-canon-cr2",
            "cr3" => "image/x-canon-cr3",
            "nef" => "image/x-nikon-nef",
            "nrw" => "image/x-nikon-nrw",
            "raf" => "image/x-fuji-raf",
            "rw2" => "image/x-panasonic-rw2",
            "orf" => "image/x-olympus-orf",
            "pef" => "image/x-pentax-pef",
            "srw" => "image/x-samsung-srw",
            "dng" => "image/x-adobe-dng",
            "tif" | "tiff" => "image/tiff",
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "heic" | "heif" => "image/heic",
            _ => "application/octet-stream",
        }
        .to_string()
    }

    fn extract_image_info(
        path: &Path,
        mime_type: &str,
        options: &ScanOptions,
    ) -> Result<(u32, u32, Option<ExifData>, Option<Vec<u8>>, Option<Vec<u8>>, bool)> {
        let is_raw = matches!(
            mime_type,
            "image/x-sony-arw"
                | "image/x-canon-cr2"
                | "image/x-canon-cr3"
                | "image/x-nikon-nef"
                | "image/x-nikon-nrw"
                | "image/x-fuji-raf"
                | "image/x-panasonic-rw2"
                | "image/x-olympus-orf"
                | "image/x-pentax-pef"
                | "image/x-samsung-srw"
                | "image/x-adobe-dng"
        );

        let mut width = 0;
        let mut height = 0;
        let mut exif_data = None;
        let mut embedded_preview = None;
        let mut thumbnail = None;

        if is_raw {
            // Use zenraw to get metadata and embedded preview
            let data = fs::read(path)?;
            let config = zenraw::RawDecodeConfig::default()
                .with_output(zenraw::OutputMode::Linear)
                .with_max_pixels(200_000_000);
            
            if let Ok(output) = zenraw::decode(&data, &config, &enough::Unstoppable) {
                width = output.info.width;
                height = output.info.height;
                
                exif_data = Some(ExifData {
                    make: Some(output.info.make.clone()),
                    model: Some(output.info.model.clone()),
                    lens: None,
                    focal_length: None,
                    aperture: None,
                    shutter_speed: None,
                    iso: None,
                    date_taken: None,
                    exposure_compensation: None,
                    white_balance: None,
                    orientation: Some(output.info.orientation),
                    gps: None,
                    color_space: None,
                    exposure_program: None,
                    metering_mode: None,
                    flash: None,
                    focal_length_35mm: None,
                });

                // Extract embedded preview if available
                if options.extract_embedded_preview {
                    if let Ok(preview) = Self::extract_embedded_preview_zenraw(&data) {
                        embedded_preview = Some(preview);
                    }
                }
            }
        } else {
            // For JPEG/TIFF/PNG, use kamadak_exif
            #[cfg(feature = "exif")]
            {
                let file = fs::File::open(path)?;
                let exif = kamadak_exif::Exif::from_reader(std::io::BufReader::new(file))?;
                exif_data = Self::parse_exif(&exif);
            }
            
            #[cfg(not(feature = "exif"))]
            {
                exif_data = None;
            }
            
            // Get dimensions from image crate
            if let Ok(img) = image::open(path) {
                width = img.width();
                height = img.height();
            }

            // Generate thumbnail
            if options.generate_thumbnails {
                thumbnail = Self::generate_thumbnail(path, options.thumbnail_max_dimension).ok();
            }
        }

        // Generate thumbnail from embedded preview if no thumbnail yet
        if thumbnail.is_none() && embedded_preview.is_some() && options.generate_thumbnails {
            if let Some(preview_bytes) = &embedded_preview {
                thumbnail = Self::generate_thumbnail_from_bytes(preview_bytes, options.thumbnail_max_dimension).ok();
            }
        }

        Ok((width, height, exif_data, embedded_preview, thumbnail, is_raw))
    }

    fn extract_embedded_preview_zenraw(data: &[u8]) -> Result<Vec<u8>> {
        // zenraw doesn't directly expose embedded preview extraction
        // We'll use a separate approach for RAW embedded previews
        // For now, return empty - will implement with rawloader or custom TIFF parsing
        Ok(Vec::new())
    }

    #[cfg(feature = "exif")]
    fn parse_exif(exif: &kamadak_exif::Exif) -> Option<ExifData> {
        let get_string = |tag: Tag| -> Option<String> {
            exif.get(tag).and_then(|v| v.to_string().parse().ok())
        };

        let get_rational = |tag: Tag| -> Option<String> {
            exif.get(tag).and_then(|v| {
                if let kamadak_exif::Value::Rational(num, denom) = v {
                    Some(format!("{}/{}", num, denom))
                } else {
                    None
                }
            })
        };

        Some(ExifData {
            make: get_string(Tag::Make),
            model: get_string(Tag::Model),
            lens: get_string(Tag::LensModel),
            focal_length: get_rational(Tag::FocalLength),
            aperture: get_rational(Tag::FNumber),
            shutter_speed: get_rational(Tag::ExposureTime),
            iso: get_string(Tag::PhotographicSensitivity).or_else(|| get_string(Tag::ISOSpeedRatings)),
            date_taken: get_string(Tag::DateTimeOriginal).or_else(|| get_string(Tag::DateTime)),
            exposure_compensation: get_rational(Tag::ExposureBiasValue),
            white_balance: get_string(Tag::WhiteBalance),
            orientation: exif.get(Tag::Orientation).and_then(|v| v.to_u32()),
            gps: Self::parse_gps(exif),
            color_space: get_string(Tag::ColorSpace),
            exposure_program: get_string(Tag::ExposureProgram),
            metering_mode: get_string(Tag::MeteringMode),
            flash: get_string(Tag::Flash),
            focal_length_35mm: get_rational(Tag::FocalLengthIn35mmFilm),
        })
    }

    #[cfg(not(feature = "exif"))]
    fn parse_exif(_exif: &kamadak_exif::Exif) -> Option<ExifData> {
        None
    }

    #[cfg(feature = "exif")]
    fn parse_gps(exif: &kamadak_exif::Exif) -> Option<GpsData> {
        let lat_ref = exif.get(Tag::GPSLatitudeRef)?;
        let lat = exif.get(Tag::GPSLatitude)?;
        let lon_ref = exif.get(Tag::GPSLongitudeRef)?;
        let lon = exif.get(Tag::GPSLongitude)?;
        let alt = exif.get(Tag::GPSAltitude);

        fn dms_to_deg(dms: &kamadak_exif::Value, ref_val: &kamadak_exif::Value) -> Option<f64> {
            if let (kamadak_exif::Value::Rational(dms), kamadak_exif::Value::Ascii(ref_bytes)) = (dms, ref_val) {
                if dms.len() == 3 && !ref_bytes.is_empty() {
                    let deg = dms[0].0 as f64 / dms[0].1 as f64;
                    let min = dms[1].0 as f64 / dms[1].1 as f64;
                    let sec = dms[2].0 as f64 / dms[2].1 as f64;
                    let mut deg = deg + min / 60.0 + sec / 3600.0;
                    if ref_bytes[0] == b'S' || ref_bytes[0] == b'W' {
                        deg = -deg;
                    }
                    return Some(deg);
                }
            }
            None
        }

        let latitude = dms_to_deg(
            &exif.get(Tag::GPSLatitude)?,
            &exif.get(Tag::GPSLatitudeRef)?,
        )?;
        let longitude = dms_to_deg(
            &exif.get(Tag::GPSLongitude)?,
            &exif.get(Tag::GPSLongitudeRef)?,
        )?;

        let altitude = exif.get(Tag::GPSAltitude).and_then(|a| {
            if let kamadak_exif::Value::Rational(num, denom) = a {
                Some(num as f64 / denom as f64)
            } else {
                None
            }
        });

        Some(GpsData { latitude, longitude, altitude })
    }

    #[cfg(not(feature = "exif"))]
    fn parse_gps(_exif: &kamadak_exif::Exif) -> Option<GpsData> {
        None
    }

    fn generate_thumbnail(path: &Path, max_dim: u32) -> Result<Vec<u8>> {
        let img = image::open(path)?;
        let thumb = img.thumbnail(max_dim, max_dim);
        let mut bytes = Vec::new();
        thumb.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)?;
        Ok(bytes)
    }

    fn generate_thumbnail_from_bytes(data: &[u8], max_dim: u32) -> Result<Vec<u8>> {
        let img = image::load_from_memory(data)?;
        let thumb = img.thumbnail(max_dim, max_dim);
        let mut bytes = Vec::new();
        thumb.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)?;
        Ok(bytes)
    }
}

impl From<ScannedImage> for LibraryImage {
    fn from(s: ScannedImage) -> Self {
        Self {
            id: 0,
            path: s.path,
            filename: s.filename,
            sha256: s.sha256,
            width: s.width,
            height: s.height,
            file_size: s.file_size,
            modified_time: s.modified_time,
            imported_time: s.imported_time,
            rating: s.rating,
            color_label: s.color_label,
            pick_reject: s.pick_reject,
            has_raw: s.has_raw,
            mime_type: s.mime_type,
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ScanStats {
    pub total_files: usize,
    pub imported: usize,
    pub errors: usize,
    pub with_embedded_preview: usize,
    pub with_thumbnails: usize,
    pub duration_ms: u64,
}

// XMP Sidecar handling
pub mod xmp {
    use super::*;
    use quick_xml::events::{BytesStart, Event};
    use quick_xml::Writer;
    use std::io::Cursor;

    pub fn read_xmp_sidecar(path: &Path) -> Result<XmpMetadata> {
        let xmp_path = path.with_extension("xmp");
        if !xmp_path.exists() {
            return Ok(XmpMetadata::default());
        }
        let content = fs::read_to_string(&xmp_path)?;
        parse_xmp(&content)
    }

    pub fn write_xmp_sidecar(path: &Path, metadata: &XmpMetadata) -> Result<()> {
        let xmp_path = path.with_extension("xmp");
        let xml = generate_xmp(metadata)?;
        fs::write(&xmp_path, xml)?;
        Ok(())
    }

    #[derive(Debug, Default, Clone, Serialize, Deserialize)]
    pub struct XmpMetadata {
        pub rating: Option<i16>,           // -1 to 5 (XMP:Rating)
        pub label: Option<String>,         // Color label (XMP:Label)
        pub pick_reject: Option<i16>,      // Private: cullimingo namespace
        pub title: Option<String>,
        pub description: Option<String>,
        pub keywords: Vec<String>,
        pub creator: Option<String>,
        pub copyright: Option<String>,
    }

    fn parse_xmp(content: &str) -> Result<XmpMetadata> {
        let mut metadata = XmpMetadata::default();
        let mut reader = quick_xml::Reader::from_str(content);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name = std::str::from_utf8(e.name().as_bytes())?;
                    match name {
                        "xmp:Rating" | "Rating" => {
                            if let Ok(Event::Text(t)) = reader.read_event_into(&mut buf) {
                                let s = quick_xml::escape::unescape(t.as_ref()).ok();
                                metadata.rating = s.and_then(|s| s.parse().ok());
                            }
                        }
                        "xmp:Label" | "Label" => {
                            if let Ok(Event::Text(t)) = reader.read_event_into(&mut buf) {
                                metadata.label = quick_xml::escape::unescape(t.as_ref()).ok().map(|s| s.to_string());
                            }
                        }
                        "cullimingo:PickReject" | "PickReject" => {
                            if let Ok(Event::Text(t)) = reader.read_event_into(&mut buf) {
                                let s = quick_xml::escape::unescape(t.as_ref()).ok();
                                metadata.pick_reject = s.and_then(|s| s.parse().ok());
                            }
                        }
                        "dc:title" | "Title" => {
                            if let Ok(Event::Text(t)) = reader.read_event_into(&mut buf) {
                                metadata.title = quick_xml::escape::unescape(t.as_ref()).ok().map(|s| s.to_string());
                            }
                        }
                        "dc:description" | "Description" => {
                            if let Ok(Event::Text(t)) = reader.read_event_into(&mut buf) {
                                metadata.description = quick_xml::escape::unescape(t.as_ref()).ok().map(|s| s.to_string());
                            }
                        }
                        "dc:creator" | "Creator" => {
                            if let Ok(Event::Text(t)) = reader.read_event_into(&mut buf) {
                                metadata.creator = quick_xml::escape::unescape(t.as_ref()).ok().map(|s| s.to_string());
                            }
                        }
                        "dc:rights" | "Rights" => {
                            if let Ok(Event::Text(t)) = reader.read_event_into(&mut buf) {
                                metadata.copyright = quick_xml::escape::unescape(t.as_ref()).ok().map(|s| s.to_string());
                            }
                        }
                        "dc:subject" | "Subject" => {
                            if let Ok(Event::Text(t)) = reader.read_event_into(&mut buf) {
                                let s = quick_xml::escape::unescape(t.as_ref()).unwrap_or_default();
                                metadata.keywords.push(s.to_string());
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(_)) => {}
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(metadata)
    }

    fn generate_xmp(metadata: &XmpMetadata) -> Result<String> {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        
        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
        writer.write_event(Event::Start(BytesStart::new("x:xmpmeta")))?;
        writer.write_event(Event::Start(BytesStart::new("rdf:RDF")))?;
        writer.write_event(Event::Start(BytesStart::new("rdf:Description")))?;
        
        if let Some(rating) = metadata.rating {
            write_simple(&mut writer, "xmp:Rating", &rating.to_string())?;
        }
        if let Some(label) = &metadata.label {
            write_simple(&mut writer, "xmp:Label", label)?;
        }
        if let Some(pr) = metadata.pick_reject {
            write_simple(&mut writer, "cullimingo:PickReject", &pr.to_string())?;
        }
        if let Some(title) = &metadata.title {
            write_simple(&mut writer, "dc:title", title)?;
        }
        if let Some(desc) = &metadata.description {
            write_simple(&mut writer, "dc:description", desc)?;
        }
        if let Some(creator) = &metadata.creator {
            write_simple(&mut writer, "dc:creator", creator)?;
        }
        if let Some(copyright) = &metadata.copyright {
            write_simple(&mut writer, "dc:rights", copyright)?;
        }
        for kw in &metadata.keywords {
            write_simple(&mut writer, "dc:subject", kw)?;
        }
        
        writer.write_event(Event::End(BytesEnd::new("rdf:Description")))?;
        writer.write_event(Event::End(BytesEnd::new("rdf:RDF")))?;
        writer.write_event(Event::End(BytesEnd::new("x:xmpmeta")))?;
        
        let result = writer.into_inner().into_inner();
        Ok(String::from_utf8(result)?)
    }

    fn write_simple(writer: &mut Writer<Cursor<Vec<u8>>>, name: &str, value: &str) -> Result<()> {
        writer.write_event(Event::Start(BytesStart::new(name)))?;
        writer.write_event(Event::Text(value.into()))?;
        writer.write_event(Event::End(BytesEnd::new(name)))?;
        Ok(())
    }
}