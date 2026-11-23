//! Renders three frames of each `.tgs` in a folder into one PNG (a check).
fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("folder");
    let out = args.next().expect("output png");
    let size = 128u32;
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "tgs"))
        .collect();
    files.sort();
    let mut sheet = tiny_skia::Pixmap::new(size * 3, size * files.len() as u32).unwrap();
    sheet.fill(tiny_skia::Color::from_rgba8(40, 44, 52, 255));
    for (row, path) in files.iter().enumerate() {
        let bytes = std::fs::read(path).unwrap();
        let a = match tdr_lottie::Animation::from_tgs(&bytes) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                continue;
            }
        };
        let n = a.frame_count();
        for (col, frame) in [0, n / 3, 2 * n / 3].into_iter().enumerate() {
            let t = std::time::Instant::now();
            let p = a.render(frame, size, size).unwrap();
            if col == 1 {
                eprintln!("{} {:?} frames {n}", path.display(), t.elapsed());
            }
            sheet.draw_pixmap(
                (col as u32 * size) as i32,
                (row as u32 * size) as i32,
                p.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
    sheet.save_png(out).unwrap();
}
