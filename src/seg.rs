//! Background removal: runs a small U2-Net ONNX model with `tract` (pure Rust, no DLLs,
//! no Python, no GPU drivers). Only ONE model file is downloaded, on first use.
use image::{imageops, GrayImage, RgbImage};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use tract_onnx::prelude::*;

pub struct SegModel {
    pub key: &'static str,
    pub label: &'static str,
    pub size_mb: u32,
    /// square network input size, and the per-channel normalisation the model was trained with
    pub input: usize,
    pub std: [f32; 3],
}

pub const MODELS: &[SegModel] = &[
    SegModel {
        key: "u2net_human_seg",
        label: "Best for people (U2Net human, 168 MB)",
        size_mb: 168,
        input: 320,
        std: [0.229, 0.224, 0.225],
    },
    SegModel {
        key: "isnet_general",
        label: "Sharp hair & edges (IS-Net, 170 MB)",
        size_mb: 170,
        input: 1024,
        std: [1.0, 1.0, 1.0],
    },
];

pub fn model_by_key(key: &str) -> &'static SegModel {
    MODELS.iter().find(|m| m.key == key).unwrap_or(&MODELS[0])
}

/// %LOCALAPPDATA%\RiponComputer  (same folder as the Python app)
pub fn app_dir() -> PathBuf {
    crate::models::app_dir()
}

pub fn model_path(key: &str) -> PathBuf {
    crate::models::find(key).unwrap_or_else(|| crate::models::entry(key).map(|e| crate::models::dest_path(&e)).unwrap_or_default())
}

pub fn is_installed(key: &str) -> bool {
    crate::models::is_installed(key)
}

pub fn download(key: &str, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<(), String> {
    crate::models::download(key, progress, cancel)
}

type Runner = Box<dyn Fn(Tensor) -> TractResult<Vec<f32>> + Send + Sync>;
static CACHE: OnceLock<Mutex<Vec<(String, std::sync::Arc<Runner>)>>> = OnceLock::new();

/// Loads the model in the background at start-up, so the first "Remove background" is fast.
pub fn warm_up(key: &str) {
    if !is_installed(key) {
        return;
    }
    let key = key.to_string();
    std::thread::spawn(move || {
        let _ = std::panic::catch_unwind(|| load_runner(&key));
    });
}

fn load_runner(key: &str) -> Result<std::sync::Arc<Runner>, String> {
    let cell = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    // a crash inside one earlier job must never block every later job
    let mut g = cell.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, r)) = g.iter().find(|(k, _)| k == key) {
        return Ok(r.clone());
    }
    let path = model_path(key);
    let meta = std::fs::metadata(&path).map_err(|_| format!("The AI model '{key}' is not installed. Open 'AI Models' and download it, or Import the file.\n{}", path.display()))?;
    if let Some(e) = crate::models::entry(key) {
        if meta.len() < e.min_bytes {
            return Err(format!("The AI model file looks incomplete ({} bytes). Delete it in 'AI Models' and download it again:\n{}", meta.len(), path.display()));
        }
    }
    // some exports carry fixed shape info, others symbolic dims: try the strict way first, then the tolerant one
    let n = model_by_key(key).input;
    let load = |tolerant: bool| -> TractResult<_> {
        let mut inf = tract_onnx::onnx();
        if tolerant {
            inf = inf.with_ignore_value_info(true).with_ignore_output_shapes(true);
        }
        inf.model_for_path(&path)?.with_input_fact(0, InferenceFact::dt_shape(f32::datum_type(), tvec!(1, 3, n, n)))?.into_optimized()?.into_runnable()
    };
    let plan = load(false)
        .or_else(|_| load(true))
        .map_err(|e| format!("Could not load the AI model ({e:#}). Delete the file in 'AI Models' and download / import it again:\n{}", path.display()))?;
    let r: Runner = Box::new(move |t: Tensor| {
        // use all CPU cores for the heavy convolution / matrix work
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
        let ex = tract_linalg::multithread::Executor::multithread(n);
        let out = tract_linalg::multithread::multithread_tract_scope(ex, || plan.run(tvec!(t.into())))?;
        let v = out[0].to_plain_array_view::<f32>()?;
        Ok(v.iter().copied().collect())
    });
    let r = std::sync::Arc::new(r);
    g.retain(|(k, _)| k != key);
    g.push((key.to_string(), r.clone()));
    Ok(r)
}

/// Returns a soft alpha matte (0..255) with the SAME aspect as `img`, longest side <= `max_side`.
/// (The app refines it against the final crop later, so edges stay sharp.)
pub fn segment(img: &RgbImage, key: &str, max_side: u32) -> Result<GrayImage, String> {
    let runner = load_runner(key)?;
    let (w, h) = img.dimensions();
    let k = (max_side as f32 / w.max(h) as f32).min(1.0);
    let (aw, ah) = (((w as f32 * k).round() as u32).max(8), ((h as f32 * k).round() as u32).max(8));
    let work = if k < 1.0 { imageops::resize(img, aw, ah, imageops::FilterType::Triangle) } else { img.clone() };
    let m = model_by_key(key);
    let n = m.input;
    let small = imageops::resize(&work, n as u32, n as u32, imageops::FilterType::Triangle);
    let maxv = small.pixels().flat_map(|p| p.0).max().unwrap_or(255).max(1) as f32;
    let mean = [0.485f32, 0.456, 0.406];
    let std = m.std;
    let mut data = vec![0f32; 3 * n * n];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            data[c * n * n + i] = (p[c] as f32 / maxv - mean[c]) / std[c];
        }
    }
    let t = Tensor::from_shape(&[1, 3, n, n], &data).map_err(|e| e.to_string())?;
    let out = runner(t).map_err(|e| format!("AI model failed: {e}"))?;
    if out.len() < n * n {
        return Err("Unexpected model output".into());
    }
    let (mn, mx) = out[..n * n].iter().fold((f32::MAX, f32::MIN), |a, v| (a.0.min(*v), a.1.max(*v)));
    let rng = (mx - mn).max(1e-6);
    let mut g = GrayImage::new(n as u32, n as u32);
    for (i, p) in g.pixels_mut().enumerate() {
        p.0 = [(((out[i] - mn) / rng) * 255.0 + 0.5).clamp(0.0, 255.0) as u8];
    }
    Ok(imageops::resize(&g, aw, ah, imageops::FilterType::CatmullRom))
}
