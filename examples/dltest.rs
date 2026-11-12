use ripon_studio::models;
use std::sync::atomic::AtomicBool;
fn main() {
    let c = AtomicBool::new(false);
    // 1) a bad first link falls through to the working second link
    let mut r = models::registry();
    let e = r.iter_mut().find(|e| e.id == "span_x4").unwrap();
    e.urls = vec!["https://github.com/facefusion/facefusion-assets/releases/download/models/gone.onnx".into(), e.urls[0].clone()];
    models::save_registry(&r).unwrap();
    println!("download: {:?}", models::download("span_x4", &|_, _| {}, &c));
    println!("installed: {}", models::is_installed("span_x4"));
    // 2) only a dead link -> readable error
    let mut r = models::registry();
    r.iter_mut().find(|e| e.id == "clear_reality_x4").unwrap().urls = vec!["https://github.com/facefusion/facefusion-assets/releases/download/models/gone.onnx".into()];
    models::save_registry(&r).unwrap();
    println!("dead: {:?}", models::download("clear_reality_x4", &|_, _| {}, &c));
    // 3) backup + restore
    let z = std::env::temp_dir().join("ripon_bk.zip");
    println!("backup: {:?}", models::backup(&z));
    models::delete("span_x4");
    println!("after delete: {}", models::is_installed("span_x4"));
    println!("restore: {:?}", models::restore(&z));
    println!("after restore: {}", models::is_installed("span_x4"));
    println!("links kept: {:?}", models::entry("clear_reality_x4").unwrap().urls);
}
