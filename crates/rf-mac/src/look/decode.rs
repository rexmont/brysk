//! Image decoding for DirectLook, the macOS half of `rf-win::look::decode`.
//!
//! ImageIO decodes at the requested resolution and applies EXIF orientation. The `image`
//! crate is the fallback for extra formats; camera fields remain available in the details card.

use std::path::Path;

use crate::thumbs::Bgra;
use core_foundation::{base::{CFType, TCFType}, boolean::CFBoolean, dictionary::{CFDictionary, CFDictionaryRef}, number::CFNumber, string::{CFString, CFStringRef}, url::CFURL};
use std::ffi::c_void;

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithURL(url: *const c_void, options: CFDictionaryRef) -> *const c_void;
    fn CGImageSourceCopyPropertiesAtIndex(source: *const c_void, index: usize, options: CFDictionaryRef) -> CFDictionaryRef;
    fn CGImageSourceCreateThumbnailAtIndex(source: *const c_void, index: usize, options: CFDictionaryRef) -> *const c_void;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFStringRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFStringRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFStringRef;
    static kCGImageSourceShouldCacheImmediately: CFStringRef;
    static kCGImagePropertyPixelWidth: CFStringRef;
    static kCGImagePropertyPixelHeight: CFStringRef;
    static kCGImagePropertyOrientation: CFStringRef;
}

fn source(path: &Path) -> Option<CFType> {
    let url = CFURL::from_path(path, false)?;
    let ptr = unsafe { CGImageSourceCreateWithURL(url.as_CFTypeRef(), std::ptr::null()) };
    (!ptr.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(ptr) })
}

fn properties(source: &CFType) -> Option<(u32, u32, u8)> {
    unsafe {
        let ptr = CGImageSourceCopyPropertiesAtIndex(source.as_CFTypeRef(), 0, std::ptr::null());
        if ptr.is_null() { return None; }
        let props = CFDictionary::<CFString, CFType>::wrap_under_create_rule(ptr);
        let number = |key| props.find(CFString::wrap_under_get_rule(key)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64());
        let w = u32::try_from(number(kCGImagePropertyPixelWidth)?).ok()?;
        let h = u32::try_from(number(kCGImagePropertyPixelHeight)?).ok()?;
        (w > 0 && h > 0).then_some((w, h, number(kCGImagePropertyOrientation).unwrap_or(1) as u8))
    }
}

/// Native codecs decode directly at the requested resolution and apply the EXIF transform.
fn native(path: &Path, max_side: u32) -> Option<(Bgra, u32, u32, u8)> {
    let source = source(path)?;
    let (w, h, orientation) = properties(&source)?;
    unsafe {
        let options = CFDictionary::from_CFType_pairs(&[
            (CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailFromImageAlways), CFBoolean::true_value().as_CFType()),
            (CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailWithTransform), CFBoolean::true_value().as_CFType()),
            (CFString::wrap_under_get_rule(kCGImageSourceShouldCacheImmediately), CFBoolean::true_value().as_CFType()),
            (CFString::wrap_under_get_rule(kCGImageSourceThumbnailMaxPixelSize), CFNumber::from(i64::from(max_side.max(1))).as_CFType()),
        ]);
        let ptr = CGImageSourceCreateThumbnailAtIndex(source.as_CFTypeRef(), 0, options.as_concrete_TypeRef());
        if ptr.is_null() { return None; }
        let image = CFType::wrap_under_create_rule(ptr);
        let pixels = crate::thumbs::cg_image_to_bgra(image.as_CFTypeRef().cast_mut(), max_side)?;
        let (w, h) = if (5..=8).contains(&orientation) { (h, w) } else { (w, h) };
        Some((pixels, w, h, orientation))
    }
}

/// Only called on Direct Look's dedicated worker, never on a shared executor thread.
pub fn prioritize_worker() {
    unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0); }
}

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
    source(path).and_then(|s| properties(&s)).map(|(w, h, o)| if (5..=8).contains(&o) { (h, w) } else { (w, h) })
        .or_else(|| image::image_dimensions(path).ok().filter(|(w, h)| *w > 0 && *h > 0))
}

/// Decodes the first frame, at most `max_side` pixels on its longer side, with its EXIF facts.
pub fn decode_full(path: &Path, max_side: u32) -> anyhow::Result<Decoded> {
    let (pixels, w, h, native_orientation) = match native(path, max_side) {
        Some(decoded) => decoded,
        None => {
            let img = image::open(path)?;
            let (w, h) = (img.width(), img.height());
            (rgba(img.thumbnail(max_side.max(1), max_side.max(1)).to_rgba8()), w, h, 1)
        }
    };
    let mut orientation = native_orientation;
    let mut exif = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        if let Ok(r) = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)) {
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
    Ok(Decoded { frames: vec![(pixels, 100)], orientation, exif, width: w, height: h })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("brysk-look-{}-{name}", std::process::id()))
    }

    #[test]
    fn native_decode_keeps_resolution_colour_alpha_and_orientation() {
        let path = scratch("pixels.png");
        let image = image::RgbaImage::from_fn(64, 32, |x, y| {
            if y < 16 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, x as u8, 255, 128]) }
        });
        image.save(&path).unwrap();
        let decoded = decode_full(&path, 8192).unwrap();
        assert_eq!((decoded.width, decoded.height), (64, 32));
        let b = &decoded.frames[0].0;
        assert_eq!((b.width, b.height), (64, 32));
        assert_eq!(&b.data[..4], &[0, 0, 255, 255]);
        assert_eq!(b.data[(20 * 64 * 4) + 3], 128);
        assert_eq!(native(&path, 16).map(|(b, ..)| (b.width, b.height)), Some((16, 8)));
        let _ = std::fs::remove_file(path);

        let path = scratch("rotated.jpg");
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image).write_to(&mut bytes, image::ImageFormat::Jpeg).unwrap();
        // EXIF orientation 6 (90° clockwise), one SHORT in an otherwise empty TIFF IFD.
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(exif);
        jpeg.extend_from_slice(&bytes.get_ref()[2..]);
        std::fs::write(&path, jpeg).unwrap();
        assert_eq!(dimensions(&path), Some((32, 64)));
        let decoded = decode_full(&path, 8192).unwrap();
        assert_eq!((decoded.width, decoded.height), (32, 64));
        assert_eq!((decoded.frames[0].0.width, decoded.frames[0].0.height), (32, 64));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[ignore = "manual 24 MP decode timing"]
    fn time_original_photo_decode() {
        let path = scratch("24mp.jpg");
        image::RgbImage::from_fn(6000, 4000, |x, y| image::Rgb([(x % 251) as u8, (y % 251) as u8, ((x + y) % 251) as u8])).save(&path).unwrap();
        let start = std::time::Instant::now();
        let old = image::open(&path).unwrap().thumbnail(8192, 8192).to_rgba8();
        let old_time = start.elapsed();
        drop(old);
        let start = std::time::Instant::now();
        let screen = decode_full(&path, 2048).unwrap();
        eprintln!("24 MP screen image: {:?} ({} px)", start.elapsed(), screen.frames[0].0.width);
        drop(screen);
        let start = std::time::Instant::now();
        let decoded = decode_full(&path, 8192).unwrap();
        eprintln!("24 MP original: Rust {:?}, ImageIO {:?}", old_time, start.elapsed());
        assert_eq!((decoded.width, decoded.height), (6000, 4000));
        let _ = std::fs::remove_file(path);
    }
}
