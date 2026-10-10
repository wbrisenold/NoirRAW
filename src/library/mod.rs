pub mod db;
pub mod scanner;
pub mod thumbnail;
pub mod face;

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryImage {
    pub id: i64,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Face {
    pub id: i64,
    pub image_id: i64,
    pub bbox: [f32; 4], // x, y, w, h (normalized 0-1)
    pub landmarks: Option<[[f32; 2]; 5]>, // 5 facial landmarks
    pub embedding: Option<Vec<f32>>, // 512-dim ArcFace embedding
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorLabel {
    None = 0,
    Red = 1,
    Yellow = 2,
    Green = 3,
    Blue = 4,
    Purple = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PickReject {
    None = 0,
    Pick = 1,
    Reject = -1,
}

impl Default for ColorLabel {
    fn default() -> Self { ColorLabel::None }
}

impl Default for PickReject {
    fn default() -> Self { PickReject::None }
}

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "arw", "cr2", "cr3", "nef", "nrw", "raf", "rw2", "orf", "pef", "srw",
    "dng", "tif", "tiff", "jpg", "jpeg", "png", "webp", "bmp", "heic", "heif",
];

// ThumbnailData for UI (matches Slint-generated type)
#[derive(Debug, Clone)]
pub struct ThumbnailData {
    pub path: String,
    pub name: String,
    pub rating: i32,
    pub color_label: i32,
    pub pick_reject: i32,
    pub thumbnail: slint::Image,
    pub width: i32,
    pub height: i32,
    pub has_raw: bool,
    pub is_selected: bool,
    pub is_active: bool,
}

// Re-export scanner types
pub use scanner::{LibraryScanner, ScanOptions, ScanStats, ScannedImage};
pub use db::LibraryDb;
pub use thumbnail::{ThumbnailGenerator, ThumbnailCache};