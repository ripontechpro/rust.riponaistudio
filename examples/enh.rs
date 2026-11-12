//! cargo run --release --example enh -- in.png model out.png [face]
use ripon_studio::enhance::*;
use std::sync::atomic::AtomicBool;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = image::open(&a[1]).unwrap().to_rgb8();
    let cancel = AtomicBool::new(false);
    let t = std::time::Instant::now();
    let up = upscale(&img, &a[2], &|f, _| { if (f * 20.0) as i32 % 5 == 0 { eprint!("."); } }, &cancel).unwrap();
    println!("\nupscale {}x{} -> {}x{} in {:?}", img.width(), img.height(), up.width(), up.height(), t.elapsed());
    // sanity: compare to bicubic
    let bc = image::imageops::resize(&img, up.width(), up.height(), image::imageops::FilterType::CatmullRom);
    let d: f64 = up.pixels().zip(bc.pixels()).map(|(p, q)| (0..3).map(|c| (p[c] as f64 - q[c] as f64).abs()).sum::<f64>() / 3.0).sum::<f64>() / (up.width() * up.height()) as f64;
    println!("mean abs diff vs bicubic: {d:.2}");
    let mut out = up;
    if a.len() > 4 {
        let fb = FaceBox { cx: out.width() as f32 * 0.5, cy: out.height() as f32 * 0.27, side: out.width() as f32 * 0.5 };
        let t = std::time::Instant::now();
        out = restore_face(&out, fb, &a[4], 1.0, &|_, _| {}, &cancel).unwrap();
        println!("face in {:?}", t.elapsed());
    }
    out.save(&a[3]).unwrap();
}
