//! PDF pages, the macOS half of `rf-win::look::pdf`.
//!
//! PDFKit draws a page into an image at the width asked for; the pixels come back through a
//! Core Graphics bitmap context (the same `image` crate the rest of the preview uses is used
//! for fallbacks). Everything here runs on the caller's worker thread: nothing hops to the main
//! queue and nothing blocks the UI, whose only job is to draw the finished picture.

use std::path::Path;

use anyhow::anyhow;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSString, NSURL};
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

        // (PDFKit's thumbnail is made here, on the caller's worker: PDFKit reads can run off the
        // main thread. The pixels are taken from the image's CGImage in a bitmap context, also
        // here — the old main-queue TIFF encode could hold the UI for seconds on a big page.)
        let img = objc2::rc::autoreleasepool(|_| unsafe {
            page.thumbnailOfSize_forBox(
                objc2_foundation::NSSize::new(f64::from(width), f64::from(height)),
                PDFDisplayBox::MediaBox,
            )
        });
        let mut rect = objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(0., 0.),
            objc2_foundation::NSSize::new(f64::from(width), f64::from(height)),
        );
        let cg = unsafe { img.CGImageForProposedRect_context_hints(&mut rect, None, None) }
            .ok_or_else(|| anyhow!(PdfError::Unreadable))?;
        // (the page's picture is exactly the box the layout reserved: draw into that size, as the
        // old `resize_exact` did)
        crate::thumbs::cg_image_to_bgra_exact(Retained::as_ptr(&cg) as *mut std::ffi::c_void, width, height)
            .ok_or_else(|| anyhow!(PdfError::Unreadable))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pdf() -> Option<&'static Path> {
        let path = Path::new("/System/Library/Assistant/UIPlugins/MailUI.siriUIBundle/Contents/Resources/MessageListAttachmentTemplate.pdf");
        path.exists().then_some(path)
    }

    #[test]
    fn opens_and_measures_a_pdf() {
        let Some(path) = sample_pdf() else { return };
        let pdf = Pdf::open(path, None).unwrap();
        assert!(pdf.page_count() > 0);
        let (w, h) = pdf.page_size(0).unwrap();
        assert!(w > 0.0 && h > 0.0);
    }

    /// A page is rendered on the calling (test) thread, without a main-queue round trip, at the
    /// exact box the layout asks for.
    #[test]
    fn renders_a_page_off_the_main_thread() {
        let Some(path) = sample_pdf() else { return };
        let pdf = Pdf::open(path, None).unwrap();
        if pdf.page_count() == 0 {
            return;
        }
        let img = pdf.render(0, 128).unwrap();
        assert_eq!(img.width, 128);
        assert!(img.height > 0);
        assert_eq!(img.data.len(), (img.width * img.height * 4) as usize);
        assert!(img.data.iter().any(|&b| b != 0), "the page has pixels");
    }
}
