//! Image decoding for DirectLook, the macOS half of `rf-win::look::decode`.
//!
//! The same `image` crate decodes the picture; EXIF orientation is applied and the camera
//! fields are read for the details card, as Windows' decoder does.

use std::path::Path;

use crate::thumbs::Bgra;

#[derive(Debug, Clone)]
pub struct Decoded {
    pub frames: Vec<(Bgra, u32)>,
    pub orientation: u8,
    pub exif: Vec<(String, String)>,
    pub width: u32,
    pub height: u32,
}

fn rgba(img: image::RgbaImage) -> Bgra {
    let (width, height) = img.dimensions();
    let mut data = img.into_raw();
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Bgra { width, height, data }
}

/// The picture's size without decoding it (a header read).
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok().filter(|(w, h)| *w > 0 && *h > 0)
}

/// Decodes the first frame, at most `max_side` pixels on its longer side, with its EXIF facts.
pub fn decode_full(path: &Path, max_side: u32) -> anyhow::Result<Decoded> {
    let img = image::open(path)?;
    let (w, h) = (img.width(), img.height());
    let mut orientation = 1u8;
    let mut exif = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        if let Ok(mut r) = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)) {
            if let Some(f) = r.get_field(exif::Tag::Orientation, exif::In::PRIMARY) {
                orientation = f.value.get_uint(0).unwrap_or(1) as u8;
            }
            for (tag, name) in [
                (exif::Tag::Make, "Kamera"),
                (exif::Tag::Model, "Model"),
                (exif::Tag::DateTimeOriginal, "Çekim"),
                (exif::Tag::ExposureTime, "Pozlama"),
                (exif::Tag::FNumber, "Diyafram"),
                (exif::Tag::ISOSpeed, "ISO"),
                (exif::Tag::FocalLength, "Odak"),
            ] {
                if let Some(f) = r.get_field(tag, exif::In::PRIMARY) {
                    exif.push((name.to_string(), f.display_value().with_unit(&r).to_string()));
                }
            }
        }
    }
    let shown = img.thumbnail(max_side.max(1), max_side.max(1)).to_rgba8();
    Ok(Decoded { frames: vec![(rgba(shown), 100)], orientation, exif, width: w, height: h })
}