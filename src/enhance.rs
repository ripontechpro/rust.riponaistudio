//! AI enhance: tiled x2 / x4 upscaling (SPAN, ClearReality, UltraSharp, Real-ESRGAN) and
//! GFPGAN face restoration. Pure Rust (tract) - no Python, no GPU, no extra DLLs.
use crate::imgops::{crop_rgb, smoothstep};
use crate::models;
use image::{imageops, RgbImage};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tract_onnx::prelude::*;

/// input NCHW (1,3,h,w) floats -> (output floats, out_h, out_w)
type Runner = Arc<dyn Fn(Vec<f32>, usize, usize) -> TractResult<(Vec<f32>, usize, usize)> + Send + Sync>;
static CACHE: OnceLock<Mutex<Option<(String, usize, Runner)>>> = OnceLock::new();

fn runner(id: &str, side: usize) -> Result<Runner, String> {
    let cell = CACHE.get_or_init(|| Mutex::new(None));
    let mut g = cell.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, s, r)) = g.as_ref() {
        if k == id && *s == side {
            return Ok(r.clone());
        }
    }
    let path = models::find(id).ok_or_else(|| "This AI model is not installed. Open 'AI Models' and download it (or Import the file).".to_string())?;
    let two_inputs = id == "codeformer";
    let load = || -> TractResult<Runner> {
        let mut m = tract_onnx::onnx().with_ignore_value_info(true).with_ignore_output_shapes(true).model_for_path(&path)?;
        m = m.with_input_fact(0, InferenceFact::dt_shape(f32::datum_type(), tvec!(1, 3, side, side)))?;
        if two_inputs {
            m = m.with_input_fact(1, InferenceFact::dt_shape(f64::datum_type(), Vec::<usize>::new()))?;
        }
        let plan = m.into_optimized()?.into_runnable()?;
        let r: Runner = Arc::new(move |data: Vec<f32>, h: usize, w: usize| {
            let t = Tensor::from_shape(&[1, 3, h, w], &data)?;
            let out = if two_inputs { plan.run(tvec!(t.into(), tensor0(0.7f64).into()))? } else { plan.run(tvec!(t.into()))? };
            let shape = out[0].shape().to_vec();
            let v = out[0].to_plain_array_view::<f32>()?;
            Ok((v.iter().copied().collect::<Vec<f32>>(), shape[shape.len() - 2], shape[shape.len() - 1]))
        });
        Ok(r)
    };
    let r = load().map_err(|e| format!("Could not load the AI model ({e:#}).\nDelete it in 'AI Models' and download / import it again."))?;
    *g = Some((id.to_string(), side, r.clone()));
    Ok(r)
}

const TILE: usize = 64;
const PAD: usize = 8;

fn px_clamped(img: &RgbImage, x: i64, y: i64) -> [u8; 3] {
    let xx = x.clamp(0, img.width() as i64 - 1) as u32;
    let yy = y.clamp(0, img.height() as i64 - 1) as u32;
    img.get_pixel(xx, yy).0
}

/// Number of tiles for an image (for time estimates and progress).
pub fn tile_count(w: u32, h: u32) -> usize {
    let s = (TILE - 2 * PAD) as u32;
    (w.div_ceil(s) * h.div_ceil(s)) as usize
}

/// Upscale with an ONNX super-resolution model. Output is `scale` times bigger (2 or 4, read from the model).
pub fn upscale(img: &RgbImage, id: &str, progress: &dyn Fn(f32, &str), cancel: &AtomicBool) -> Result<RgbImage, String> {
    let run = runner(id, TILE)?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    let step = TILE - 2 * PAD;
    let nx = w.div_ceil(step);
    let ny = h.div_ceil(step);
    let total = nx * ny;
    let mut out: Option<(RgbImage, usize)> = None;
    let mut done = 0;
    for ty in 0..ny {
        for tx in 0..nx {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            let (x0, y0) = ((tx * step) as i64, (ty * step) as i64);
            let mut data = vec![0f32; 3 * TILE * TILE];
            for j in 0..TILE {
                for i in 0..TILE {
                    let p = px_clamped(img, x0 + i as i64 - PAD as i64, y0 + j as i64 - PAD as i64);
                    for c in 0..3 {
                        data[c * TILE * TILE + j * TILE + i] = p[c] as f32 / 255.0;
                    }
                }
            }
            let (o, oh, ow) = run(data, TILE, TILE).map_err(|e| format!("AI model failed: {e}"))?;
            let s = ow / TILE;
            if s == 0 || oh != s * TILE || o.len() < 3 * oh * ow {
                return Err("Unexpected AI model output size".into());
            }
            let (big, _) = out.get_or_insert_with(|| (RgbImage::new((w * s) as u32, (h * s) as u32), s));
            let bw = big.width() as usize;
            let bh = big.height() as usize;
            let (cw, ch) = (step.min(w - tx * step), step.min(h - ty * step));
            for j in 0..ch * s {
                for i in 0..cw * s {
                    let (ox, oy) = (tx * step * s + i, ty * step * s + j);
                    if ox >= bw || oy >= bh {
                        continue;
                    }
                    let (sx, sy) = (PAD * s + i, PAD * s + j);
                    let px = big.get_pixel_mut(ox as u32, oy as u32);
                    for c in 0..3 {
                        px[c] = (o[c * oh * ow + sy * ow + sx].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    }
                }
            }
            done += 1;
            progress(done as f32 / total as f32, "Upscaling...");
        }
    }
    out.map(|(i, _)| i).ok_or_else(|| "Empty picture".into())
}

/// Square face region in picture pixels.
#[derive(Clone, Copy, Debug)]
pub struct FaceBox {
    pub cx: f32,
    pub cy: f32,
    pub side: f32,
}

/// Restore the face inside `fb` with GFPGAN / CodeFormer (512x512) and blend it back softly.
/// `strength` 0..1 mixes restored and original.
pub fn restore_face(img: &RgbImage, fb: FaceBox, id: &str, strength: f32, progress: &dyn Fn(f32, &str), cancel: &AtomicBool) -> Result<RgbImage, String> {
    let run = runner(id, 512)?;
    progress(0.1, "Restoring face...");
    let (crop, cov) = crop_rgb(img, fb.cx, fb.cy, fb.side, fb.side, 0.0, 512, 512);
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let mut data = vec![0f32; 3 * 512 * 512];
    for (i, p) in crop.pixels().enumerate() {
        for c in 0..3 {
            data[c * 512 * 512 + i] = (p[c] as f32 / 255.0 - 0.5) / 0.5;
        }
    }
    let (o, oh, ow) = run(data, 512, 512).map_err(|e| format!("Face AI failed: {e}"))?;
    if oh != 512 || ow != 512 {
        return Err("Unexpected face model output".into());
    }
    let mut res = RgbImage::new(512, 512);
    for (i, p) in res.pixels_mut().enumerate() {
        for c in 0..3 {
            p[c] = (((o[c * 512 * 512 + i].clamp(-1.0, 1.0) + 1.0) * 0.5) * 255.0 + 0.5) as u8;
        }
    }
    let side = fb.side.round().max(8.0) as u32;
    let big = imageops::resize(&res, side, side, imageops::FilterType::Lanczos3);
    let covb = imageops::resize(&cov, side, side, imageops::FilterType::Triangle);
    let mut outimg = img.clone();
    let x0 = (fb.cx - side as f32 / 2.0).round() as i64;
    let y0 = (fb.cy - side as f32 / 2.0).round() as i64;
    let s = strength.clamp(0.0, 1.0);
    for j in 0..side {
        for i in 0..side {
            let (x, y) = (x0 + i as i64, y0 + j as i64);
            if x < 0 || y < 0 || x >= img.width() as i64 || y >= img.height() as i64 {
                continue;
            }
            let dx = (i as f32 + 0.5) / side as f32 * 2.0 - 1.0;
            let dy = (j as f32 + 0.5) / side as f32 * 2.0 - 1.0;
            let d = (dx * dx + dy * dy).sqrt();
            let wgt = (1.0 - smoothstep(0.62, 0.98, d)) * s * (covb.get_pixel(i, j)[0] as f32 / 255.0);
            if wgt <= 0.0 {
                continue;
            }
            let a = big.get_pixel(i, j);
            let b = outimg.get_pixel_mut(x as u32, y as u32);
            for c in 0..3 {
                b[c] = (b[c] as f32 + (a[c] as f32 - b[c] as f32) * wgt + 0.5) as u8;
            }
        }
    }
    progress(1.0, "Face restored");
    Ok(outimg)
}

pub const UPSCALERS: &[(&str, &str)] = &[
    ("span_x4", "Fast  (SPAN x4)"),
    ("clear_reality_x4", "Fast, photos  (ClearReality x4)"),
    ("ultra_sharp_x4", "Sharp  (UltraSharp x4)"),
    ("real_esrgan_x4", "Best, slow  (Real-ESRGAN x4)"),
    ("real_esrgan_x2", "Gentle, slow  (Real-ESRGAN x2)"),
];
pub const FACE_MODELS: &[(&str, &str)] = &[("gfpgan_1.4", "GFPGAN 1.4"), ("gfpgan_1.2", "GFPGAN 1.2 (softer)"), ("codeformer", "CodeFormer")];
