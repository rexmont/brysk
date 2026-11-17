//! PDF pages, the macOS half of `rf-win::look::pdf`.
//!
//! macOS renders PDFs with PDFKit/CoreGraphics. Until that reader is written (DirectLook
//! milestone), opening one says "unreadable" and the preview pane shows the file's details;
//! `qlmanage` can make a first-page thumbnail, which arrives with the Quick Look work.

use std::path::Path;

use anyhow::anyhow;

use crate::thumbs::Bgra;

/// Widest page we render (physical pixels); wider requests are capped.
pub const MAX_WIDTH: u32 = 4096;

/// An open PDF.
pub struct Pdf;

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
    pub fn open(_path: &Path, _password: Option<&str>) -> anyhow::Result<Pdf> {
        Err(anyhow!(PdfError::Unreadable))
    }

    pub fn page_count(&self) -> u32 {
        0
    }

    pub fn page_size(&self, _index: u32) -> anyhow::Result<(f32, f32)> {
        Err(anyhow!(PdfError::Unreadable))
    }

    pub fn render(&self, _index: u32, _width: u32) -> anyhow::Result<Bgra> {
        Err(anyhow!(PdfError::Unreadable))
    }
}