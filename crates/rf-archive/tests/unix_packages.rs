#![cfg(unix)]

use std::{fs, path::PathBuf, os::unix::fs::{symlink, PermissionsExt}};
use rf_archive::{compress_with, extract_with, CompressOptions, Format, Options};

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("brysk-archive-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[test]
fn package_roundtrips_keep_executables_and_framework_links() {
    let d = Scratch::new("roundtrip");
    let app = d.0.join("Demo.app");
    fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    fs::create_dir_all(app.join("Contents/Frameworks/Kit.framework/Versions/A")).unwrap();
    let exe = app.join("Contents/MacOS/Demo");
    fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let fw = app.join("Contents/Frameworks/Kit.framework");
    fs::write(fw.join("Versions/A/Kit"), b"library").unwrap();
    symlink("A", fw.join("Versions/Current")).unwrap();
    symlink("Versions/Current/Kit", fw.join("Kit")).unwrap();
    // A plain executable without links also exercises the parallel own-decoder path.
    for (i, (format, password)) in [
        (Format::Zip, None), (Format::Zip, Some("secret")),
        (Format::Tar, None), (Format::TarGz, None), (Format::TarZst, None),
        (Format::SevenZ, None), (Format::SevenZ, Some("secret")),
    ].into_iter().enumerate() {
        let dest = d.0.join(format!("archive-{i}.{}", format.extension()));
        let opts = CompressOptions { password: password.map(str::to_owned), ..Default::default() };
        compress_with(&[app.clone(), exe.clone()], &dest, format, 1, &opts, &mut |_| {}, &|| false).unwrap();
        let out = d.0.join(format!("out-{i}"));
        extract_with(&dest, &out, &Options { password: password.map(str::to_owned), ..Default::default() }, &mut |_| {}, &|| false).unwrap();
        for p in [out.join("Demo.app/Contents/MacOS/Demo"), out.join("Demo")] {
            assert_eq!(fs::read(&p).unwrap(), b"#!/bin/sh\nexit 0\n", "{format:?}");
            assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o755, "{format:?}");
        }
        let kit = out.join("Demo.app/Contents/Frameworks/Kit.framework");
        assert_eq!(fs::read_link(kit.join("Versions/Current")).unwrap(), PathBuf::from("A"), "{format:?}");
        assert_eq!(fs::read_link(kit.join("Kit")).unwrap(), PathBuf::from("Versions/Current/Kit"), "{format:?}");
        assert_eq!(fs::read(kit.join("Kit")).unwrap(), b"library");
    }
}

#[test]
fn extraction_cannot_write_through_a_symlink_parent() {
    use std::io::Write;
    let d = Scratch::new("safe-parent");
    let outside = d.0.join("outside");
    fs::create_dir(&outside).unwrap();
    let dest = d.0.join("out");
    fs::create_dir(&dest).unwrap();
    symlink(&outside, dest.join("redirect")).unwrap();
    let archive = d.0.join("bad.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    w.start_file("redirect/payload", zip::write::SimpleFileOptions::default()).unwrap();
    w.write_all(b"bad").unwrap();
    w.finish().unwrap();
    assert!(rf_archive::extract(&archive, &dest, &mut |_| {}, &|| false).is_err());
    assert!(!outside.join("payload").exists());
}

#[test]
fn escaping_archive_links_are_rejected() {
    let d = Scratch::new("escape-link");
    let archive = d.0.join("bad.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    w.add_symlink("escape", "../outside", zip::write::SimpleFileOptions::default()).unwrap();
    w.finish().unwrap();
    assert!(rf_archive::extract(&archive, &d.0.join("out"), &mut |_| {}, &|| false).is_err());
    assert!(fs::symlink_metadata(d.0.join("out/escape")).is_err());
}
