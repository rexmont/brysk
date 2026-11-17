//! The file clipboard, the macOS half of `rf-win::clipboard`.
//!
//! Files are put on the general pasteboard as `public.file-url` items, which the Finder and
//! every other app read; a paste reads them back. macOS has no "cut" marker on the pasteboard
//! (the Finder moves with ⌘⌥V after a copy), so a cut is a copy here and the app's own move
//! still does what the user asked inside rustyfiles.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

use crate::fileops::Transfer;
use crate::HWND;

static SEQUENCE: AtomicU32 = AtomicU32::new(1);

/// Clears the pasteboard (the owner window is a Windows concept; unused here).
pub fn clear(_owner: HWND) -> anyhow::Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Changes whenever the pasteboard is written; used to notice that a copy landed elsewhere.
pub fn sequence() -> u32 {
    SEQUENCE.load(Ordering::Relaxed)
}

/// Puts `paths` on the pasteboard as file URLs. `kind` is kept for the signature: macOS has no
/// cut state, see the module note.
pub fn set_files(_owner: HWND, paths: &[PathBuf], _kind: Transfer) -> anyhow::Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let urls: Vec<Retained<NSURL>> = paths
        .iter()
        .map(|p| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy())))
        .collect();
    let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
        urls.iter().map(|u| ProtocolObject::from_retained(u.clone())).collect();
    let arr = NSArray::from_retained_slice(&objects);
    let ok = pb.writeObjects(&arr);
    SEQUENCE.fetch_add(1, Ordering::Relaxed);
    anyhow::ensure!(ok, "pano yazılamadı");
    Ok(())
}

/// Whether the pasteboard holds files that can be pasted here.
pub fn has_files() -> bool {
    let pb = NSPasteboard::generalPasteboard();
    let types = NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]);
    pb.canReadItemWithDataConformingToTypes(&types)
}

pub fn can_paste() -> bool {
    has_files()
}

/// The files on the pasteboard and whether the Finder marks the operation a move (it does
/// not; always [`Transfer::Copy`]).
pub fn get_files(_owner: HWND) -> anyhow::Result<Option<(Vec<PathBuf>, Transfer)>> {
    let pb = NSPasteboard::generalPasteboard();
    let Some(items) = pb.pasteboardItems() else { return Ok(None) };
    let mut out = Vec::new();
    for item in items.iter() {
        if let Some(s) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) {
            let s = s.to_string();
            if let Some(p) = file_url_to_path(&s) {
                out.push(p);
            }
        }
    }
    Ok((!out.is_empty()).then_some((out, Transfer::Copy)))
}

fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // file:///Users/x → /Users/x, with %XX escapes
    let decoded: String = {
        let bytes = rest.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    Some(PathBuf::from(decoded))
}

/// Puts a picture (PNG bytes) on the pasteboard.
pub fn set_image(_owner: HWND, _w: u32, _h: u32, _rgba: &[u8], png: &[u8]) -> anyhow::Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let data = NSData::with_bytes(png);
    let ok = pb.setData_forType(Some(&data), unsafe { NSPasteboardTypePNG });
    SEQUENCE.fetch_add(1, Ordering::Relaxed);
    anyhow::ensure!(ok, "pano yazılamadı");
    Ok(())
}