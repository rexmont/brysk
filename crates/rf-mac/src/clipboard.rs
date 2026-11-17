//! The file clipboard, the macOS half of `rf-win::clipboard`.
//!
//! Files are put on the general pasteboard as `public.file-url` items, which the Finder and
//! every other app read; a paste reads them back. macOS has no "cut" marker on the pasteboard,
//! so a cut from Brysk is remembered here together with the pasteboard's change count: a paste
//! inside Brysk moves the files, while a copy (or another app's write) is a plain copy.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

use crate::fileops::Transfer;
use crate::HWND;

static SEQUENCE: AtomicU32 = AtomicU32::new(1);

/// A cut made by this app: the pasteboard change count it belongs to and the paths.
static CUT: OnceLock<Mutex<Option<(isize, Vec<PathBuf>)>>> = OnceLock::new();

fn cut() -> &'static Mutex<Option<(isize, Vec<PathBuf>)>> {
    CUT.get_or_init(|| Mutex::new(None))
}

/// Clears the pasteboard (the owner window is a Windows concept; unused here).
pub fn clear(_owner: HWND) -> anyhow::Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    *cut().lock().unwrap() = None;
    SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Changes whenever the pasteboard is written; used to notice that a copy landed elsewhere.
pub fn sequence() -> u32 {
    SEQUENCE.load(Ordering::Relaxed)
}

/// Puts `paths` on the pasteboard as file URLs. A `Move` is remembered until the pasteboard is
/// written again, so Brysk's own paste moves the files the user cut.
pub fn set_files(_owner: HWND, paths: &[PathBuf], kind: Transfer) -> anyhow::Result<()> {
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
    *cut().lock().unwrap() = (kind == Transfer::Move).then(|| (pb.changeCount(), paths.to_vec()));
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

/// The files on the pasteboard and whether Brysk cut them (a move) or they are a copy.
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
    if out.is_empty() {
        return Ok(None);
    }
    // A cut only counts while the pasteboard still holds what this app wrote.
    let kind = match cut().lock().unwrap().as_ref() {
        Some((count, paths)) if *count == pb.changeCount() && *paths == out => Transfer::Move,
        _ => Transfer::Copy,
    };
    Ok(Some((out, kind)))
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
    *cut().lock().unwrap() = None;
    SEQUENCE.fetch_add(1, Ordering::Relaxed);
    anyhow::ensure!(ok, "pano yazılamadı");
    Ok(())
}