//! Photo adjustments. All functions work on float RGB (0..1) at the FINAL photo size,
//! `mask` is the person mask (0..1) of the same size.
use crate::imgops::*;

/// Gentle auto colour + light: white balance, levels, exposure.
pub fn auto_enhance(img: &mut Fimg, mask: &[f32], strength: f32) {
    let s = strength.clamp(0.0, 1.0);
    if s <= 0.0 {
        return;
    }
    let n = img.d.len();
    let sel: Vec<usize> = (0..n).filter(|&i| mask[i] > 0.5).collect();
    let sel = if sel.len() < 200 { (0..n).collect() } else { sel };
    // 1. white balance (grey-world on mid-tones, limited)
    let (mut sum, mut cnt) = ([0f64; 3], 0f64);
    for &i in &sel {
        let l = luma(img.d[i]);
        if l > 0.12 && l < 0.9 {
            for c in 0..3 {
                sum[c] += img.d[i][c] as f64;
            }
            cnt += 1.0;
        }
    }
    if cnt > 50.0 {
        let m = [sum[0] / cnt, sum[1] / cnt, sum[2] / cnt];
        let grey = (m[0] + m[1] + m[2]) / 3.0;
        // skin is naturally red-ish, so only correct halfway and clamp the gains
        let g: Vec<f32> = (0..3).map(|c| lerp(1.0, (grey / m[c].max(1e-3)) as f32, 0.5).clamp(0.9, 1.12)).collect();
        for p in img.d.iter_mut() {
            for c in 0..3 {
                p[c] = (p[c] * lerp(1.0, g[c], s)).clamp(0.0, 1.0);
            }
        }
    }
    // 2. levels on luma percentiles
    let mut hist = [0u32; 256];
    for &i in &sel {
        hist[(luma(img.d[i]) * 255.0) as usize & 255] += 1;
    }
    let total = sel.len() as f32;
    let pct = |q: f32| -> f32 {
        let mut acc = 0f32;
        for (b, c) in hist.iter().enumerate() {
            acc += *c as f32;
            if acc >= q * total {
                return b as f32 / 255.0;
            }
        }
        1.0
    };
    let (lo, hi) = (pct(0.008), pct(0.992));
    if hi - lo > 0.25 {
        let scale = ((0.97 - 0.03) / (hi - lo)).clamp(1.0, 1.35);
        for p in img.d.iter_mut() {
            for c in 0..3 {
                let v = ((p[c] - lo) * scale + 0.03).clamp(0.0, 1.0);
                p[c] = lerp(p[c], v, s);
            }
        }
    }
    // 3. exposure: bring the median skin/person luma towards 0.52
    let mut ls: Vec<f32> = sel.iter().map(|&i| luma(img.d[i])).collect();
    ls.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = ls[ls.len() / 2].clamp(0.05, 0.95);
    let gamma = (0.52f32.ln() / med.ln()).clamp(0.75, 1.3);
    for p in img.d.iter_mut() {
        for c in 0..3 {
            p[c] = lerp(p[c], p[c].powf(gamma), s * 0.8);
        }
    }
}

pub fn tone(img: &mut Fimg, brightness: f32, contrast: f32, saturation: f32, shadows: f32) {
    let g = 1.0 / (1.0 + 0.8 * shadows.max(0.0));
    for p in img.d.iter_mut() {
        if shadows > 0.001 {
            let l = luma(*p).max(0.002);
            let k = (l.powf(g) / l).min(4.0);
            for c in 0..3 {
                p[c] = (p[c] * k).clamp(0.0, 1.0);
            }
        }
        for c in 0..3 {
            let v = (p[c] - 0.5) * (1.0 + contrast) + 0.5 + brightness * 0.25;
            p[c] = v.clamp(0.0, 1.0);
        }
        if saturation.abs() > 0.001 {
            let l = luma(*p);
            for c in 0..3 {
                p[c] = (l + (p[c] - l) * (1.0 + saturation)).clamp(0.0, 1.0);
            }
        }
    }
}

pub fn to_grey(img: &mut Fimg) {
    for p in img.d.iter_mut() {
        let l = luma(*p);
        *p = [l, l, l];
    }
}

/// Unsharp mask on luma.
pub fn sharpen(img: &mut Fimg, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let l = img.luma();
    let sigma = (img.w as f32 / 413.0 * 0.9).max(0.7);
    let bl = gauss_blur(&l, img.w, img.h, sigma);
    for (i, p) in img.d.iter_mut().enumerate() {
        let diff = l[i] - bl[i];
        let soft = diff * smoothstep(0.004, 0.03, diff.abs());
        let add = soft * amount * 1.4;
        for c in 0..3 {
            p[c] = (p[c] + add).clamp(0.0, 1.0);
        }
    }
}

/// Edge-preserving noise reduction (guided filter on each channel).
pub fn denoise(img: &mut Fimg, strength: f32) {
    if strength <= 0.0 {
        return;
    }
    let r = ((img.w as f32 / 200.0).round() as usize).clamp(1, 6);
    let eps = (0.012 + 0.07 * strength).powi(2);
    for c in 0..3 {
        let pl = img.plane(c);
        let f = guided(&pl, &pl, img.w, img.h, r, eps);
        let out: Vec<f32> = pl.iter().zip(&f).map(|(a, b)| lerp(*a, *b, strength.min(1.0))).collect();
        img.set_plane(c, &out);
    }
}

pub fn skin_mask(img: &Fimg, person: &[f32]) -> Vec<f32> {
    let m: Vec<f32> = img
        .d
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (r, g, b) = (p[0], p[1], p[2]);
            let cb = -0.168736 * r - 0.331264 * g + 0.5 * b + 0.5;
            let cr = 0.5 * r - 0.418688 * g - 0.081312 * b + 0.5;
            let l = luma(*p);
            let sb = smoothstep(0.27, 0.31, cb) * (1.0 - smoothstep(0.49, 0.53, cb));
            let sr = smoothstep(0.50, 0.53, cr) * (1.0 - smoothstep(0.68, 0.72, cr));
            sb * sr * smoothstep(0.15, 0.25, l) * person[i]
        })
        .collect();
    box_blur(&m, img.w, img.h, 2)
}

/// Natural skin smoothing: edge-aware blur on skin only, keeps some texture.
pub fn skin_smooth(img: &mut Fimg, person: &[f32], strength: f32) {
    if strength <= 0.0 {
        return;
    }
    let s = strength.clamp(0.0, 1.0);
    let mask = skin_mask(img, person);
    let r = ((img.w as f32 / 120.0).round() as usize).clamp(2, 10);
    let eps = (0.015 + 0.06 * s).powi(2);
    let keep = lerp(0.35, 0.15, s);
    for c in 0..3 {
        let pl = img.plane(c);
        let f1 = guided(&pl, &pl, img.w, img.h, r, eps);
        let f = if s > 0.55 { guided(&f1, &f1, img.w, img.h, r, eps) } else { f1 };
        let out: Vec<f32> = (0..pl.len())
            .map(|i| {
                let smooth = f[i] + (pl[i] - f[i]) * keep;
                lerp(pl[i], smooth, (mask[i] * (0.35 + 0.65 * s)).min(1.0))
            })
            .collect();
        img.set_plane(c, &out);
    }
}

/// Remove a blemish: diffusion fill of a disc from its surroundings (x,y,r in pixels).
pub fn heal(img: &mut Fimg, x: f32, y: f32, r: f32) {
    let r = r.max(2.0);
    let rr = (r + 3.0).ceil() as i32;
    let (cx, cy) = (x.round() as i32, y.round() as i32);
    let (x0, y0) = ((cx - rr).max(0), (cy - rr).max(0));
    let (x1, y1) = ((cx + rr).min(img.w as i32 - 1), (cy + rr).min(img.h as i32 - 1));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (bw, bh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
    let inside = |px: i32, py: i32| -> bool {
        let dx = px as f32 - x;
        let dy = py as f32 - y;
        dx * dx + dy * dy <= r * r
    };
    let idx = |px: i32, py: i32| (py as usize) * img.w + px as usize;
    // initialise with the mean of the ring around the disc
    let (mut sum, mut n) = ([0f32; 3], 0f32);
    for py in y0..=y1 {
        for px in x0..=x1 {
            let d2 = (px as f32 - x).powi(2) + (py as f32 - y).powi(2);
            if d2 > r * r && d2 <= (r + 3.0) * (r + 3.0) {
                let p = img.d[idx(px, py)];
                for c in 0..3 {
                    sum[c] += p[c];
                }
                n += 1.0;
            }
        }
    }
    if n < 1.0 {
        return;
    }
    let mean = [sum[0] / n, sum[1] / n, sum[2] / n];
    let mut buf = vec![[0f32; 3]; bw * bh];
    let mut fixed = vec![true; bw * bh];
    for py in y0..=y1 {
        for px in x0..=x1 {
            let k = (py - y0) as usize * bw + (px - x0) as usize;
            if inside(px, py) {
                buf[k] = mean;
                fixed[k] = false;
            } else {
                buf[k] = img.d[idx(px, py)];
            }
        }
    }
    for _ in 0..60 {
        let prev = buf.clone();
        for ly in 1..bh - 1 {
            for lx in 1..bw - 1 {
                let k = ly * bw + lx;
                if !fixed[k] {
                    for c in 0..3 {
                        buf[k][c] = 0.25 * (prev[k - 1][c] + prev[k + 1][c] + prev[k - bw][c] + prev[k + bw][c]);
                    }
                }
            }
        }
    }
    for py in y0..=y1 {
        for px in x0..=x1 {
            let k = (py - y0) as usize * bw + (px - x0) as usize;
            if !fixed[k] {
                let d = ((px as f32 - x).powi(2) + (py as f32 - y).powi(2)).sqrt();
                let t = 1.0 - smoothstep(r - 1.5, r + 0.5, d);
                let o = &mut img.d[idx(px, py)];
                for c in 0..3 {
                    o[c] = lerp(o[c], buf[k][c], t);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heal_removes_a_dark_spot() {
        let mut img = Fimg { w: 60, h: 60, d: vec![[0.8, 0.6, 0.5]; 3600] };
        for y in 28..32 {
            for x in 28..32 {
                img.d[y * 60 + x] = [0.2, 0.1, 0.1];
            }
        }
        heal(&mut img, 30.0, 30.0, 5.0);
        let c = img.d[30 * 60 + 30];
        assert!((c[0] - 0.8).abs() < 0.05, "{:?}", c);
    }
}
