//! RAW development via zenraw (rawler backend for CR3/X-Trans support).
use std::path::Path;
use crate::image_io::SceneImage;

pub fn open_raw(path: &Path) -> Result<SceneImage, String> {
    let data = std::fs::read(path).map_err(|e| format!("RAW file read: {e}"))?;

    let config = zenraw::RawDecodeConfig::default()
        .with_output(zenraw::OutputMode::Linear)
        .with_max_pixels(200_000_000);

    let output = zenraw::decode(&data, &config, &enough::Unstoppable)
        .map_err(|e| format!("RAW decode: {e}"))?;

    let w = output.pixels.width() as u32;
    let h = output.pixels.height() as u32;

    // zenraw Linear output is interleaved f32 RGB (3 floats/pixel)
    let bytes = output.pixels.as_contiguous_bytes()
        .ok_or("zenraw: non-contiguous pixel buffer")?;
    let rgb = bytemuck::cast_slice::<u8, f32>(bytes);

    if rgb.len() != (w * h * 3) as usize {
        return Err("zenraw: pixel buffer size mismatch".into());
    }

    // Convert RGB f32 to RGBA f32 (alpha=1.0) for SceneImage
    let mut rgba = Vec::with_capacity((w * h) as usize);
    for chunk in rgb.chunks_exact(3) {
        rgba.push([chunk[0], chunk[1], chunk[2], 1.0]);
    }

    Ok(SceneImage { width: w, height: h, pixels: rgba })
}