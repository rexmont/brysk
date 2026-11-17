//! Legacy text encodings, the macOS half of `rf-win::encoding`.
//!
//! Old Turkish text is Windows-1254, not UTF-8. macOS has no "system ANSI code page": the
//! file manager decodes as UTF-8 when valid, otherwise tries the user's legacy page
//! (Turkish when the locale says so), then falls back to a lossy decode.

use std::sync::OnceLock;

/// On macOS there is no ANSI code page; a Turkish user gets 1254, everyone else 1252 (the
/// same default Windows uses in Western locales).
pub fn legacy_ansi_codepage() -> u32 {
    static CP: OnceLock<u32> = OnceLock::new();
    *CP.get_or_init(|| if user_locale_is_turkish() { 1254 } else { 1252 })
}

fn decode(raw: &[u8], cp: u32) -> Option<String> {
    if raw.is_empty() {
        return Some(String::new());
    }
    let to = c"UTF-8";
    let from = std::ffi::CString::new(format!("CP{cp}")).ok()?;
    unsafe {
        let cd = libc::iconv_open(to.as_ptr(), from.as_ptr());
        if cd as isize == -1 {
            return None;
        }
        let mut out = vec![0u8; raw.len() * 3 + 8];
        let mut in_ptr = raw.as_ptr() as *mut libc::c_char;
        let mut in_left = raw.len();
        let mut out_ptr = out.as_mut_ptr() as *mut libc::c_char;
        let mut out_left = out.len();
        let n = libc::iconv(cd, &mut in_ptr, &mut in_left, &mut out_ptr, &mut out_left);
        libc::iconv_close(cd);
        if n == usize::MAX {
            return None;
        }
        out.truncate(out.len() - out_left);
        String::from_utf8(out).ok()
    }
}

/// Text from bytes of unknown encoding: UTF-8 if valid (a truncated trailing sequence is
/// tolerated, since previews read only the head of a file), otherwise the legacy page.
pub fn decode_text(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(e) if e.error_len().is_none() => String::from_utf8_lossy(&raw[..e.valid_up_to()]).into_owned(),
        Err(_) => decode(raw, legacy_ansi_codepage()).unwrap_or_else(|| String::from_utf8_lossy(raw).into_owned()),
    }
}

/// True when the user's language is Turkish (sorting then treats `I` as capital `ı`).
pub fn user_locale_is_turkish() -> bool {
    static TR: OnceLock<bool> = OnceLock::new();
    *TR.get_or_init(|| {
        for var in ["LC_ALL", "LC_CTYPE", "LANG"] {
            if let Ok(v) = std::env::var(var) {
                if v.to_ascii_lowercase().starts_with("tr") {
                    return true;
                }
            }
        }
        let loc = std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleLocale"]).output().ok();
        loc.map(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().starts_with("tr")).unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_1254_and_utf8() {
        let raw = [0xDE, b'i', b'r', b'k', b'e', b't', b' ', 0xFD, 0xF0, 0xFC, 0xFE, 0xF6, 0xE7];
        assert_eq!(decode(&raw, 1254).as_deref(), Some("Şirket ığüşöç"));
        assert_eq!(decode_text("Şirket".as_bytes()), "Şirket");
        let mut cut = "abcş".as_bytes().to_vec();
        cut.pop();
        assert_eq!(decode_text(&cut), "abc");
    }
}