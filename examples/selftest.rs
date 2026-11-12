//! Headless self-test:  cargo run --release --example selftest -- <photo> <outdir>
use ripon_studio::{pipeline::*, seg, sheet};
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let src = image::open(&a[1]).unwrap().to_rgb8();
    let out = std::path::Path::new(&a[2]);
    std::fs::create_dir_all(out).unwrap();
    let key = a.get(3).map(|s| s.as_str()).unwrap_or("u2net_human_seg");
    let t = Instant::now();
    let alpha = seg::segment(&src, key, 1024).expect("segment");
    println!("segment: {:?}  alpha {}x{}", t.elapsed(), alpha.width(), alpha.height());
    alpha.save(out.join("alpha.png")).unwrap();
    let mut e = Edit::default();
    e.reset_crop(src.width(), src.height());
    println!("auto_crop ok = {}", e.auto_crop(&alpha, src.width(), src.height()));
    println!("crop cx {:.0} cy {:.0} cw {:.0}", e.cx, e.cy, e.cw);
    e.bg = [30, 120, 200];
    e.skin = 0.5;
    e.caption = "RIPON AI PHOTO STUDIO".into();
    e.caption_date = "08-11-2026".into();
    let t = Instant::now();
    let ph = render_photo(&src, Some(&alpha), &e, None);
    println!("render {}x{} in {:?}", ph.width(), ph.height(), t.elapsed());
    let cap = add_caption(&ph, &e);
    cap.save(out.join("photo.png")).unwrap();
    let o = sheet::SheetOpts { copies: 8, ..Default::default() };
    let (l, c, p) = sheet::plan(cap.width(), cap.height(), &o, 300);
    println!("layout {}x{} per page {} copies {} pages {} rotated {}", l.cols, l.rows, l.per_page(), c, p, l.rotated);
    let pages = sheet::make_sheets(&cap, &o, 300).unwrap();
    std::fs::write(out.join("sheet.jpg"), sheet::encode_jpeg(&pages[0], 92, 300)).unwrap();
    std::fs::write(out.join("sheet.png"), sheet::encode_png(&pages[0], 300)).unwrap();
    std::fs::write(out.join("sheet.pdf"), sheet::encode_pdf(&pages, 300, 92)).unwrap();
    let (b, q, fits) = sheet::jpeg_under_kb(&ph, Some((354, 472)), 50, 300);
    println!("online: {} bytes q{} fits {}", b.len(), q, fits);
    std::fs::write(out.join("online.jpg"), b).unwrap();
}
