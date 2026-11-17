//! PDF pages, the macOS half of `rf-win::look::pdf`.
//!
//! PDFKit draws a page into an image at the width asked for; the pixels come back through the
//! same `image` crate the rest of the preview uses. Renders run through the main queue because
//! PDFKit's drawing is AppKit work; the decode of the resulting image happens off it.

use std::path::Path;

use anyhow::anyhow;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSString, NSURL};
use objc2_pdf_kit::{PDFDisplayBox, PDFDocument};

use crate::thumbs::Bgra;

/// Widest page we render (physical pixels); wider requests are capped.
pub const MAX_WIDTH: u32 = 4096;

/// An open PDF.
pub struct Pdf {
    doc: Retained<PDFDocument>,
}

/// Why a PDF can't be shown.
#[derive(Debug, PartialEq, Eq)]
pub enum PdfError {
    /// Protected with a password.
    Password,
    /// Not readable as a PDF (damaged, or not a PDF).
    Unreadable,
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PdfError::Password => "the PDF is protected with a password",
            PdfError::Unreadable => "the file can't be read as a PDF",
        })
    }
}

impl std::error::Error for PdfError {}

impl Pdf {
    pub fn open(path: &Path, password: Option<&str>) -> anyhow::Result<Pdf> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let doc = unsafe { PDFDocument::initWithURL(PDFDocument::alloc(), &url) }.ok_or_else(|| anyhow!(PdfError::Unreadable))?;
        if unsafe { doc.isLocked() } {
            let ok = password.is_some_and(|p| unsafe { doc.unlockWithPassword(&NSString::from_str(p)) });
            if !ok {
                return Err(anyhow!(PdfError::Password));
            }
        }
        Ok(Pdf { doc })
    }

    pub fn page_count(&self) -> u32 {
        unsafe { self.doc.pageCount() as u32 }
    }

    pub fn page_size(&self, index: u32) -> anyhow::Result<(f32, f32)> {
        let page = unsafe { self.doc.pageAtIndex(index as usize) }.ok_or_else(|| anyhow!(PdfError::Unreadable))?;
        let r = unsafe { page.boundsForBox(PDFDisplayBox::MediaBox) };
        Ok((r.size.width as f32, r.size.height as f32))
    }

    pub fn render(&self, index: u32, width: u32) -> anyhow::Result<Bgra> {
        let (w, h) = self.page_size(index)?;
        let width = width.clamp(32, MAX_WIDTH);
        let height = ((width as f32) * (h / w.max(1.0))).round().max(1.0) as u32;
        let page = unsafe { self.doc.pageAtIndex(index as usize) }.ok_or_else(|| anyhow!(PdfError::Unreadable))?;

        let (tx, rx) = std::sync::mpsc::channel::<Option<Vec<u8>>>();
        objc2::rc::autoreleasepool(|_| {
            let img = unsafe { page.thumbnailOfSize_forBox(objc2_foundation::NSSize::new(width as f64, height as f64), PDFDisplayBox::MediaBox) };
            let ptr = Retained::into_raw(img) as usize;
            dispatch2::DispatchQueue::main().exec_async(move || {
                if MainThreadMarker::new().is_none() {
                    let _ = tx.send(None);
                    return;
                }
                let img = unsafe { Retained::from_raw(ptr as *mut objc2_app_kit::NSImage) };
                let data = img.and_then(|i| i.TIFFRepresentation()).map(|d| d.to_vec());
                let _ = tx.send(data);
            });
        });
        let data = rx.recv_timeout(std::time::Duration::from_secs(20)).ok().flatten().ok_or_else(|| anyhow!(PdfError::Unreadable))?;
        let img = image::load_from_memory(&data)?;
        let img = if img.width() != width || img.height() != height { img.resize_exact(width, height, image::imageops::FilterType::Triangle) } else { img };
        let mut rgba = img.to_rgba8();
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        Ok(Bgra { width, height, data: rgba.into_raw() })
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_measures_a_pdf() {
        let path = Path::new("/System/Library/Assistant/UIPlugins/MailUI.siriUIBundle/Contents/Resources/MessageListAttachmentTemplate.pdf");
        if !path.exists() {
            return;
        }
        let pdf = Pdf::open(path, None).unwrap();
        assert!(pdf.page_count() > 0);
        let (w, h) = pdf.page_size(0).unwrap();
        assert!(w > 0.0 && h > 0.0);
    }
}
