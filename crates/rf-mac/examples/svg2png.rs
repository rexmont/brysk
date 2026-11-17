//! Renders an SVG at a square size: `svg2png <in.svg> <out.png> <px>` (used for the app icon).
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (input, output, size) = (&a[1], &a[2], a[3].parse::<u32>().unwrap());
    let data = std::fs::read(input).unwrap();
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).unwrap();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).unwrap();
    let s = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut pixmap.as_mut());
    pixmap.save_png(output).unwrap();
}
