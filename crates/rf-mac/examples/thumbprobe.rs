use std::path::Path;

use rf_mac::thumbs::{Kind, get_image};

fn main() {
    let Some(p) = std::env::args().nth(1) else {
        eprintln!("usage: thumbprobe <path>");
        return;
    };
    for kind in [Kind::Icon, Kind::Thumbnail] {
        match get_image(Path::new(&p), 128, kind) {
            Ok(b) => println!("{kind:?}: {}x{}, {} bytes", b.width, b.height, b.data.len()),
            Err(e) => println!("{kind:?}: error {e:#}"),
        }
    }
}
