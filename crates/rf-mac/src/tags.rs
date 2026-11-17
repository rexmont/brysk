//! Finder tags: the `com.apple.metadata:_kMDItemUserTags` extended attribute.
//!
//! A binary property list: an array of "Name\n<colour index>" strings, the index being the
//! Finder's own colour (1 grey, 2 green, 3 purple, 4 blue, 5 yellow, 6 red, 7 orange). Read and
//! written here with Foundation's property-list serializer; an item without tags has no
//! attribute at all.

use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSData, NSPropertyListFormat, NSPropertyListSerialization, NSString};

const XATTR: &str = "com.apple.metadata:_kMDItemUserTags";

fn cpath(p: &Path) -> Option<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes()).ok()
}

/// The tags on `path`: (name, Finder colour index). Empty when there are none.
pub fn read(path: &Path) -> Vec<(String, u8)> {
    let (Some(p), Ok(name)) = (cpath(path), std::ffi::CString::new(XATTR)) else { return Vec::new() };
    unsafe {
        let size = libc::getxattr(p.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0);
        if size <= 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; size as usize];
        let n = libc::getxattr(p.as_ptr(), name.as_ptr(), buf.as_mut_ptr() as *mut _, buf.len(), 0, 0);
        if n <= 0 {
            return Vec::new();
        }
        buf.truncate(n as usize);
        let data = NSData::with_bytes(&buf);
        let Ok(plist) = NSPropertyListSerialization::propertyListWithData_options_format_error(&data, objc2_foundation::NSPropertyListMutabilityOptions(1), std::ptr::null_mut()) else {
            return Vec::new();
        };
        let arr: Retained<NSArray<NSString>> = Retained::cast_unchecked(plist);
        let mut out = Vec::new();
        for i in 0..arr.count() {
            let s = arr.objectAtIndex(i);
            let text = s.to_string();
            match text.rsplit_once('\n') {
                Some((name, idx)) => out.push((name.to_string(), idx.trim().parse().unwrap_or(0))),
                None => out.push((text, 0)),
            }
        }
        out
    }
}

/// Replaces the tags on `path` (an empty slice removes the attribute).
pub fn write(path: &Path, tags: &[(String, u8)]) -> std::io::Result<()> {
    let (Some(p), Ok(name)) = (cpath(path), std::ffi::CString::new(XATTR)) else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "path"));
    };
    if tags.is_empty() {
        unsafe {
            let r = libc::removexattr(p.as_ptr(), name.as_ptr(), 0);
            if r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOATTR) {
                return Ok(());
            }
            return Err(std::io::Error::last_os_error());
        }
    }
    let strings: Vec<Retained<NSString>> = tags.iter().map(|(n, c)| NSString::from_str(&format!("{n}\n{c}"))).collect();
    let array = NSArray::from_retained_slice(&strings);
    let any: &AnyObject = unsafe { &*(&*array as *const NSArray<NSString> as *const AnyObject) };
    let data = unsafe {
        NSPropertyListSerialization::dataWithPropertyList_format_options_error(any, NSPropertyListFormat::BinaryFormat_v1_0, 0)
    }
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "property list"))?;
    let bytes = data.to_vec();
    unsafe {
        let r = libc::setxattr(p.as_ptr(), name.as_ptr(), bytes.as_ptr() as *const _, bytes.len(), 0, 0);
        if r == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finder_tags_round_trip() {
        let p = std::env::temp_dir().join(format!("rf-mac-tags-{}.txt", std::process::id()));
        std::fs::write(&p, b"x").unwrap();
        assert!(read(&p).is_empty());
        let tags = vec![("Red".to_string(), 6u8), ("İş".to_string(), 4u8)];
        write(&p, &tags).expect("write");
        assert_eq!(read(&p), tags);
        write(&p, &[]).expect("clear");
        assert!(read(&p).is_empty());
        let _ = std::fs::remove_file(&p);
    }
}
