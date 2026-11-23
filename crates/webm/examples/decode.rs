//! Decodes a WebM and writes every 5th frame side by side as raw RGBA (a check).
fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("webm");
    let output = args.next().expect("output .rgba");
    let bytes = std::fs::read(&input).unwrap();
    let t = std::time::Instant::now();
    let d = tdr_webm::decode(&bytes, 300).unwrap();
    eprintln!(
        "{}x{} frames {} in {:?}",
        d.width,
        d.height,
        d.frames.len(),
        t.elapsed()
    );
    let picked: Vec<_> = d.frames.iter().step_by(5).take(6).collect();
    let (w, h) = (d.width as usize, d.height as usize);
    let mut out = vec![0u8; w * picked.len() * h * 4];
    for (k, f) in picked.iter().enumerate() {
        for y in 0..h {
            let src = &f.rgba[y * w * 4..(y + 1) * w * 4];
            let dst = (y * w * picked.len() + k * w) * 4;
            out[dst..dst + w * 4].copy_from_slice(src);
        }
    }
    std::fs::write(&output, &out).unwrap();
    println!("{} {}", w * picked.len(), h);
}
