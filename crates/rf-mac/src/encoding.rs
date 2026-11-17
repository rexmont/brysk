//! Legacy text encodings and the user's locale, the macOS half of `rf-win::encoding`.
//!
//! Old Turkish text is Windows-1254, not UTF-8. macOS has no "system ANSI code page": the
//! file manager decodes as UTF-8 when valid, otherwise tries the user's legacy page
//! (Turkish when the preferred language says so), then falls back to a lossy decode.
//!
//! Two different macOS settings are read here, and they must not be confused:
//! * `AppleLanguages` is the ORDERED list of preferred languages. Its first entry is the UI
//!   language (and the language that decides encoding and collation), never a secondary one.
//! * `AppleLocale` is the region. It only orders dates, times and numbers.

use std::sync::OnceLock;

/// On macOS there is no ANSI code page; a Turkish user gets 1254, everyone else 1252 (the
/// same default Windows uses in Western locales).
pub fn legacy_ansi_codepage() -> u32 {
    static CP: OnceLock<u32> = OnceLock::new();
    *CP.get_or_init(|| if user_locale_is_turkish() { 1254 } else { 1252 })
}

/// Runs `defaults` (read-only) and returns its standard output when it succeeded.
fn defaults(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/defaults").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The first language of a raw `AppleLanguages` preference value.
///
/// macOS stores an ordered list: only its first entry is the user's preferred language, so a
/// secondary language (Turkish after English, for example) must never decide the UI. Both
/// shapes the system prints are accepted: the old-style plist of `defaults read`
/// (`(en-TR, tr-TR)`, with quotes where needed) and the XML of `defaults export`.
pub fn first_preferred_language(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.contains("<string") || raw.contains("<array") || raw.contains("<plist") {
        return first_xml_string(raw);
    }
    let body = raw.find('(').map_or(raw, |i| &raw[i + 1..]).trim_start();
    let first = if body.starts_with('"') {
        quoted(body)?.0
    } else {
        body.split([',', ')', '\n']).next().unwrap_or("").trim().to_string()
    };
    let first = first.trim().to_string();
    (!first.is_empty()).then_some(first)
}

/// The first `<string>` of an XML plist, after the `AppleLanguages` key when the whole global
/// domain was exported. XML entities are decoded.
fn first_xml_string(raw: &str) -> Option<String> {
    const KEY: &str = "<key>AppleLanguages</key>";
    let rest = if let Some(i) = raw.find(KEY) {
        &raw[i + KEY.len()..]
    } else if raw.contains("<key>") {
        // A whole-domain export with no `AppleLanguages`: another key's array is not ours.
        return None;
    } else {
        raw
    };
    let array = rest.find("<array")?;
    let rest = &rest[array..];
    let head = &rest[..=rest.find('>')?];
    if head.ends_with("/>") {
        return None;
    }
    // stop at the end of this array: an empty `AppleLanguages` must not borrow the next one
    let rest = &rest[head.len()..];
    let rest = &rest[..rest.find("</array>").unwrap_or(rest.len())];
    let open = rest.find("<string")?;
    let body = rest[open + "<string".len()..].strip_prefix('>')?;
    let end = body.find("</string>")?;
    let text = decode_xml_entities(body[..end].trim());
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

/// Decodes the XML entities a plist string may contain (`&amp;`, `&lt;`, `&#233;`, ...).
fn decode_xml_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(j) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..j];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32),
            _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=j]),
        }
        rest = &tail[j + 1..];
    }
    out.push_str(rest);
    out
}

/// Reads one quoted old-style plist string (the input starts at the opening quote) and returns
/// its value with the backslash escapes (`\"`, `\\`, `\n`, `\U00fc`, ...) decoded.
fn quoted(s: &str) -> Option<(String, &str)> {
    let s = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut it = s.char_indices();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => return Some((out, &s[i + 1..])),
            '\\' => {
                let (_, e) = it.next()?;
                match e {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'U' | 'u' => {
                        let mut hex = String::with_capacity(4);
                        for _ in 0..4 {
                            hex.push(it.next()?.1);
                        }
                        out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                    }
                    other => out.push(other),
                }
            }
            c => out.push(c),
        }
    }
    None
}

/// The first entry of the ordered preferred-language list (`AppleLanguages`), exactly as macOS
/// names it ("en-TR", "tr-TR", "zh-Hans-CN"). `None` when the setting is missing.
pub fn preferred_language_tag() -> Option<String> {
    defaults(&["read", "-g", "AppleLanguages"])
        .and_then(|raw| first_preferred_language(&raw))
        .or_else(|| defaults(&["export", "-g", "-"]).and_then(|raw| first_preferred_language(&raw)))
}

/// The user's region (`AppleLocale`, e.g. "en_TR"), which dates and numbers follow. This is NOT
/// the UI language; see [`preferred_language_tag`]. Falls back to `LC_TIME`/`LANG`. Read once:
/// the date formatter asks for it for every row it draws.
pub fn regional_locale() -> &'static str {
    static LOCALE: OnceLock<String> = OnceLock::new();
    LOCALE
        .get_or_init(|| {
            if let Some(s) = defaults(&["read", "-g", "AppleLocale"]) {
                let s = s.trim().trim_matches('"').trim().to_string();
                if !s.is_empty() {
                    return s;
                }
            }
            for var in ["LC_TIME", "LANG"] {
                if let Ok(v) = std::env::var(var) {
                    let v = v.split('.').next().unwrap_or(&v).replace('_', "-");
                    if !v.is_empty() && v != "C" && v != "POSIX" {
                        return v;
                    }
                }
            }
            String::new()
        })
        .as_str()
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

/// True when the user's preferred language is Turkish (sorting then treats `I` as capital `ı`,
/// and legacy text is read as Windows-1254). This is the language, not the region: an English
/// user in Türkiye keeps English collation.
pub fn user_locale_is_turkish() -> bool {
    static TR: OnceLock<bool> = OnceLock::new();
    *TR.get_or_init(|| {
        if let Some(tag) = preferred_language_tag() {
            return tag.to_ascii_lowercase().starts_with("tr");
        }
        for var in ["LC_ALL", "LC_CTYPE", "LANG"] {
            if let Ok(v) = std::env::var(var) {
                if v.to_ascii_lowercase().starts_with("tr") {
                    return true;
                }
            }
        }
        false
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

    #[test]
    fn the_first_preferred_language_wins() {
        // the exact shape `defaults read -g AppleLanguages` prints
        assert_eq!(first_preferred_language("(\n    en,\n    tr\n)").as_deref(), Some("en"));
        assert_eq!(first_preferred_language("(tr)").as_deref(), Some("tr"));
        assert_eq!(first_preferred_language("(\"en-TR\", \"tr-TR\")").as_deref(), Some("en-TR"));
        assert_eq!(first_preferred_language("(  \"zh-Hans-CN\" , en )").as_deref(), Some("zh-Hans-CN"));
        assert_eq!(first_preferred_language("(\"tr-TR\", en)").as_deref(), Some("tr-TR"), "the second entry never wins");
        assert_eq!(first_preferred_language("()"), None);
        assert_eq!(first_preferred_language("   "), None);
    }

    #[test]
    fn quoted_and_escaped_preferred_languages() {
        assert_eq!(first_preferred_language(r#"("en-US", tr)"#).as_deref(), Some("en-US"));
        assert_eq!(first_preferred_language(r#"("en\-US", tr)"#).as_deref(), Some("en-US"), "an unknown escape keeps its character");
        assert_eq!(first_preferred_language(r#"("\"tr\"", en)"#).as_deref(), Some("\"tr\""), "escaped quotes belong to the value");
        assert_eq!(first_preferred_language(r#"("\U0074\U0072", en)"#).as_deref(), Some("tr"), "old-style plist \\U escapes");
        assert_eq!(first_preferred_language(r#"("\\en", tr)"#).as_deref(), Some("\\en"), "a backslash is not an escape");
        assert_eq!(first_preferred_language("(\"en"), None, "an unterminated string names no language");
    }

    #[test]
    fn the_xml_export_names_the_same_first_language() {
        let exported = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>AppleLocale</key>
    <string>en_TR</string>
    <key>AppleLanguages</key>
    <array>
        <string>en-TR</string>
        <string>tr-TR</string>
    </array>
</dict>
</plist>"#;
        assert_eq!(first_preferred_language(exported).as_deref(), Some("en-TR"), "AppleLocale must not be taken for a language");
        let array_only = "<plist version=\"1.0\"><array><string>tr</string><string>en</string></array></plist>";
        assert_eq!(first_preferred_language(array_only).as_deref(), Some("tr"));
        assert_eq!(first_preferred_language("<array><string>en&amp;x</string></array>").as_deref(), Some("en&x"));
        let other_key_only = "<dict><key>Other</key><array><string>tr</string></array></dict>";
        assert_eq!(first_preferred_language(other_key_only), None, "only AppleLanguages names the UI language");
        let empty = "<dict><key>AppleLanguages</key><array/></dict>";
        assert_eq!(first_preferred_language(empty), None);
    }
}