//! Document / ID-card tool: find the card in a photo, straighten it with a perspective warp
//! (corners can always be corrected by hand), clean it up, and place card(s) on a sheet.
use crate::adjust;
use crate::imgops::*;
use image::{imageops, GrayImage, Rgb, RgbImage};
use serde::{Deserialize, Serialize};

pub type Pt = [f32; 2];

pub struct DocSize {
    pub name: &'static str,
    pub w_mm: f32,
    pub h_mm: f32,
}
pub const DOC_SIZES: &[DocSize] = &[
    DocSize { name: "ID card / NID / licence  (85.6 x 54 mm)", w_mm: 85.6, h_mm: 54.0 },
    DocSize { name: "Old NID / large card  (88 x 62 mm)", w_mm: 88.0, h_mm: 62.0 },
    DocSize { name: "Passport page  (125 x 88 mm)", w_mm: 125.0, h_mm: 88.0 },
    DocSize { name: "A6  (148 x 105 mm)", w_mm: 148.0, h_mm: 105.0 },
    DocSize { name: "A4 document  (210 x 297 mm)", w_mm: 210.0, h_mm: 297.0 },
    DocSize { name: "Legal document  (216 x 356 mm)", w_mm: 215.9, h_mm: 355.6 },
    DocSize { name: "Letter document  (216 x 279 mm)", w_mm: 215.9, h_mm: 279.4 },
    DocSize { name: "A5 document  (148 x 210 mm)", w_mm: 148.0, h_mm: 210.0 },
    DocSize { name: "Custom (mm)", w_mm: 85.6, h_mm: 54.0 },
];
pub const DOC_CUSTOM: usize = 8;

// ------------------------------------------------------------------ homography
/// Solve the 3x3 projective map taking `from[i]` to `to[i]` (8 unknowns, 4 point pairs).
pub fn solve_homography(from: [Pt; 4], to: [Pt; 4]) -> Option<[f64; 9]> {
    let mut a = [[0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = (from[i][0] as f64, from[i][1] as f64);
        let (u, v) = (to[i][0] as f64, to[i][1] as f64);
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for c in 0..8 {
        let mut p = c;
        for r in c + 1..8 {
            if a[r][c].abs() > a[p][c].abs() {
                p = r;
            }
        }
        if a[p][c].abs() < 1e-10 {
            return None;
        }
        a.swap(c, p);
        for r in 0..8 {
            if r != c {
                let f = a[r][c] / a[c][c];
                for k in c..9 {
                    a[r][k] -= f * a[c][k];
                }
            }
        }
    }
    let mut h = [0f64; 9];
    for i in 0..8 {
        h[i] = a[i][8] / a[i][i];
    }
    h[8] = 1.0;
    Some(h)
}

#[inline]
fn apply(h: &[f64; 9], x: f32, y: f32) -> (f32, f32) {
    let (x, y) = (x as f64, y as f64);
    let w = h[6] * x + h[7] * y + h[8];
    (((h[0] * x + h[1] * y + h[2]) / w) as f32, ((h[3] * x + h[4] * y + h[5]) / w) as f32)
}

/// Cyclic shift of the corner order = rotate the result by `k` x 90 degrees.
pub fn rotate_quad(q: [Pt; 4], k: u8) -> [Pt; 4] {
    let k = (k % 4) as usize;
    [q[k % 4], q[(k + 1) % 4], q[(k + 2) % 4], q[(k + 3) % 4]]
}

fn dist(a: Pt, b: Pt) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

pub fn quad_wh(q: &[Pt; 4]) -> (f32, f32) {
    ((dist(q[0], q[1]) + dist(q[3], q[2])) / 2.0, (dist(q[0], q[3]) + dist(q[1], q[2])) / 2.0)
}

/// 0..3 so that the warped card comes out landscape (or portrait) like the chosen size.
pub fn auto_rot(q: &[Pt; 4], want_landscape: bool) -> u8 {
    let (w, h) = quad_wh(q);
    if (w >= h) == want_landscape {
        0
    } else {
        1
    }
}

/// Order 4 points as TL, TR, BR, BL.
pub fn order_quad(p: [Pt; 4]) -> [Pt; 4] {
    let mut idx = [0usize, 1, 2, 3];
    let cx = p.iter().map(|a| a[0]).sum::<f32>() / 4.0;
    let cy = p.iter().map(|a| a[1]).sum::<f32>() / 4.0;
    idx.sort_by(|&a, &b| {
        let aa = (p[a][1] - cy).atan2(p[a][0] - cx);
        let bb = (p[b][1] - cy).atan2(p[b][0] - cx);
        aa.partial_cmp(&bb).unwrap()
    });
    // angles ascend clockwise on screen (y down) starting from -pi (left): rotate so TL is first
    let o = [p[idx[0]], p[idx[1]], p[idx[2]], p[idx[3]]];
    let mut best = 0;
    let mut bs = f32::MAX;
    for (i, q) in o.iter().enumerate() {
        let s = q[0] + q[1];
        if s < bs {
            bs = s;
            best = i;
        }
    }
    [o[best], o[(best + 1) % 4], o[(best + 2) % 4], o[(best + 3) % 4]]
}

/// Perspective-correct the quad (TL,TR,BR,BL in picture pixels) into an `ow x oh` picture.
pub fn warp_quad(img: &RgbImage, quad: [Pt; 4], ow: u32, oh: u32) -> RgbImage {
    let (qw, _) = quad_wh(&quad);
    let (mut src, mut q) = (None::<RgbImage>, quad);
    if qw / ow as f32 > 1.8 {
        // shrink first so the result is smooth, not aliased
        let k = (ow as f32 * 1.4 / qw).clamp(0.05, 1.0);
        let xs: Vec<f32> = quad.iter().map(|p| p[0]).collect();
        let ys: Vec<f32> = quad.iter().map(|p| p[1]).collect();
        let x0 = (xs.iter().cloned().fold(f32::MAX, f32::min) - 2.0).floor().max(0.0);
        let y0 = (ys.iter().cloned().fold(f32::MAX, f32::min) - 2.0).floor().max(0.0);
        let x1 = (xs.iter().cloned().fold(f32::MIN, f32::max) + 2.0).ceil().min(img.width() as f32);
        let y1 = (ys.iter().cloned().fold(f32::MIN, f32::max) + 2.0).ceil().min(img.height() as f32);
        if x1 - x0 > 8.0 && y1 - y0 > 8.0 {
            let sub = imageops::crop_imm(img, x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32).to_image();
            let (nw, nh) = (((sub.width() as f32 * k) as u32).max(2), ((sub.height() as f32 * k) as u32).max(2));
            src = Some(imageops::resize(&sub, nw, nh, imageops::FilterType::Lanczos3));
            let (kx, ky) = (nw as f32 / sub.width() as f32, nh as f32 / sub.height() as f32);
            for p in q.iter_mut() {
                *p = [(p[0] - x0) * kx, (p[1] - y0) * ky];
            }
        }
    }
    let base = src.as_ref().unwrap_or(img);
    let rect = [[0.0, 0.0], [ow as f32, 0.0], [ow as f32, oh as f32], [0.0, oh as f32]];
    let mut out = RgbImage::new(ow, oh);
    let Some(h) = solve_homography(rect, q) else { return out };
    let (bw, bh) = (base.width() as i64, base.height() as i64);
    let raw = base.as_raw();
    for v in 0..oh {
        for u in 0..ow {
            let (sx, sy) = apply(&h, u as f32 + 0.5, v as f32 + 0.5);
            let (sx, sy) = (sx - 0.5, sy - 0.5);
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let cl = |a: i64, m: i64| a.clamp(0, m - 1) as usize;
            let (xa, xb, ya, yb) = (cl(x0 as i64, bw), cl(x0 as i64 + 1, bw), cl(y0 as i64, bh), cl(y0 as i64 + 1, bh));
            let px = out.get_pixel_mut(u, v);
            for c in 0..3 {
                let g = |x: usize, y: usize| raw[(y * bw as usize + x) * 3 + c] as f32;
                let top = g(xa, ya) + (g(xb, ya) - g(xa, ya)) * fx;
                let bot = g(xa, yb) + (g(xb, yb) - g(xa, yb)) * fx;
                px[c] = (top + (bot - top) * fy + 0.5) as u8;
            }
        }
    }
    out
}

// ------------------------------------------------------------------ card detection
fn otsu(vals: &[f32], bins: usize) -> f32 {
    let mut hist = vec![0f64; bins];
    for v in vals {
        hist[((v.clamp(0.0, 0.9999)) * bins as f32) as usize] += 1.0;
    }
    let total: f64 = hist.iter().sum();
    let sum_all: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * c).sum();
    let (mut wb, mut sb, mut best, mut thr) = (0.0, 0.0, -1.0, 0);
    for (i, c) in hist.iter().enumerate() {
        wb += c;
        if wb == 0.0 {
            continue;
        }
        let wf = total - wb;
        if wf == 0.0 {
            break;
        }
        sb += i as f64 * c;
        let (mb, mf) = (sb / wb, (sum_all - sb) / wf);
        let between = wb * wf * (mb - mf) * (mb - mf);
        if between > best {
            best = between;
            thr = i;
        }
    }
    (thr as f32 + 1.0) / bins as f32
}

/// Largest connected blob of a boolean mask (4-neighbourhood).
fn largest_blob(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut label = vec![0u32; w * h];
    let (mut best_id, mut best_n, mut next) = (0u32, 0usize, 1u32);
    let mut stack = Vec::new();
    for s in 0..w * h {
        if !mask[s] || label[s] != 0 {
            continue;
        }
        let mut n = 0;
        label[s] = next;
        stack.push(s);
        while let Some(i) = stack.pop() {
            n += 1;
            let (x, y) = (i % w, i / w);
            let mut push = |j: usize| {
                if mask[j] && label[j] == 0 {
                    label[j] = next;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        if n > best_n {
            best_n = n;
            best_id = next;
        }
        next += 1;
    }
    label.iter().map(|&l| l == best_id && best_id != 0).collect()
}

fn fit_line(pts: &[(f32, f32)]) -> Option<(f32, f32, f32, f32)> {
    // returns (px, py, dx, dy): a point and unit direction (total least squares)
    if pts.len() < 6 {
        return None;
    }
    let n = pts.len() as f32;
    let (mx, my) = (pts.iter().map(|p| p.0).sum::<f32>() / n, pts.iter().map(|p| p.1).sum::<f32>() / n);
    let (mut sxx, mut sxy, mut syy) = (0f32, 0f32, 0f32);
    for p in pts {
        let (dx, dy) = (p.0 - mx, p.1 - my);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let th = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    Some((mx, my, th.cos(), th.sin()))
}

fn intersect(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> Option<Pt> {
    let det = a.2 * b.3 - a.3 * b.2;
    if det.abs() < 1e-4 {
        return None;
    }
    let t = ((b.0 - a.0) * b.3 - (b.1 - a.1) * b.2) / det;
    Some([a.0 + a.2 * t, a.1 + a.3 * t])
}

fn poly_area(q: &[Pt; 4]) -> f32 {
    let mut s = 0.0;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        s += a[0] * b[1] - b[0] * a[1];
    }
    s.abs() / 2.0
}

/// Quad (TL,TR,BR,BL in mask pixels) around the biggest blob of `mask`, plus how well the blob
/// fills the quad (1.0 = perfect rectangle).
pub fn quad_from_mask(mask: &[bool], w: usize, h: usize) -> Option<([Pt; 4], f32)> {
    let blob = largest_blob(mask, w, h);
    let area = blob.iter().filter(|b| **b).count() as f32;
    if area < (w * h) as f32 * 0.04 {
        return None;
    }
    // boundary pixels
    let mut bd: Vec<(f32, f32)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if !blob[y * w + x] {
                continue;
            }
            let edge = x == 0 || y == 0 || x + 1 == w || y + 1 == h || !blob[y * w + x - 1] || !blob[y * w + x + 1] || !blob[(y - 1) * w + x] || !blob[(y + 1) * w + x];
            if edge {
                bd.push((x as f32 + 0.5, y as f32 + 0.5));
            }
        }
    }
    if bd.len() < 20 {
        return None;
    }
    // extreme points along the diagonals -> first guess of the corners
    let ext = |f: &dyn Fn(&(f32, f32)) -> f32, max: bool| -> Pt {
        let mut best = bd[0];
        for p in &bd {
            if (f(p) > f(&best)) == max && f(p) != f(&best) {
                best = *p;
            }
        }
        [best.0, best.1]
    };
    let mut q = [ext(&|p| p.0 + p.1, false), ext(&|p| p.0 - p.1, true), ext(&|p| p.0 + p.1, true), ext(&|p| p.0 - p.1, false)];
    let diag = ((w * w + h * h) as f32).sqrt();
    // refine: fit a line to the boundary pixels of each side and intersect neighbours
    let mut lines = [None; 4];
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let (ux, uy) = (dx / len, dy / len);
        let mut pts: Vec<(f32, f32)> = bd
            .iter()
            .copied()
            .filter(|p| {
                let (rx, ry) = (p.0 - a[0], p.1 - a[1]);
                let t = (rx * ux + ry * uy) / len;
                let d = (rx * -uy + ry * ux).abs();
                (0.15..0.85).contains(&t) && d < diag * 0.05
            })
            .collect();
        for _ in 0..2 {
            if let Some(l) = fit_line(&pts) {
                let res: Vec<f32> = pts.iter().map(|p| ((p.0 - l.0) * -l.3 + (p.1 - l.1) * l.2).abs()).collect();
                let mut s = res.clone();
                s.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let cut = (s[s.len() / 2] * 2.5).max(1.5);
                let keep: Vec<(f32, f32)> = pts.iter().zip(&res).filter(|(_, r)| **r <= cut).map(|(p, _)| *p).collect();
                if keep.len() >= 6 {
                    pts = keep;
                }
            }
        }
        lines[i] = fit_line(&pts);
    }
    for i in 0..4 {
        // corner i is between side i-1 and side i
        if let (Some(l0), Some(l1)) = (lines[(i + 3) % 4], lines[i]) {
            if let Some(c) = intersect(l0, l1) {
                if dist(c, q[i]) < diag * 0.12 {
                    q[i] = c;
                }
            }
        }
    }
    let qa = poly_area(&q);
    if qa < (w * h) as f32 * 0.04 {
        return None;
    }
    Some((q, (area / qa).min(1.0)))
}

fn shrink_for_detect(img: &RgbImage) -> (RgbImage, f32) {
    let m = img.width().max(img.height()) as f32;
    let k = (480.0 / m).min(1.0);
    if k >= 1.0 {
        return (img.clone(), 1.0);
    }
    (imageops::resize(img, (img.width() as f32 * k) as u32, (img.height() as f32 * k) as u32, imageops::FilterType::Triangle), k)
}

/// Colour-distance-from-background detection. Works for a card on a table / scanner bed.
pub fn detect_classic(img: &RgbImage) -> Option<([Pt; 4], f32)> {
    let (small, k) = shrink_for_detect(img);
    let (w, h) = (small.width() as usize, small.height() as usize);
    let f = Fimg::from_rgb(&small);
    // background colour = median of a thin ring along the picture border
    let ring = (w.min(h) as f32 * 0.03).ceil().max(2.0) as usize;
    let mut ch: [Vec<f32>; 3] = [vec![], vec![], vec![]];
    for y in 0..h {
        for x in 0..w {
            if x < ring || y < ring || x + ring >= w || y + ring >= h {
                for c in 0..3 {
                    ch[c].push(f.d[y * w + x][c]);
                }
            }
        }
    }
    let mut bg = [0f32; 3];
    for c in 0..3 {
        ch[c].sort_by(|a, b| a.partial_cmp(b).unwrap());
        bg[c] = ch[c][ch[c].len() / 2];
    }
    let d: Vec<f32> = f.d.iter().map(|p| ((p[0] - bg[0]).powi(2) + (p[1] - bg[1]).powi(2) + (p[2] - bg[2]).powi(2)).sqrt() / 1.732).collect();
    let d = box_blur(&d, w, h, 1);
    // threshold: Otsu, but low enough to keep dark / background-like parts of the card (e.g. a green
    // header on a brown table). The floor comes from how noisy the background ring itself is.
    let mut ringd: Vec<f32> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if x < ring || y < ring || x + ring >= w || y + ring >= h {
                ringd.push(d[y * w + x]);
            }
        }
    }
    ringd.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p98 = ringd[(ringd.len() as f32 * 0.98) as usize % ringd.len()];
    let thr = otsu(&d, 128).min((p98 * 1.8).max(0.05)).max(0.05);
    let m: Vec<f32> = d.iter().map(|v| if *v > thr { 1.0 } else { 0.0 }).collect();
    let m = box_blur(&m, w, h, 3); // close small gaps (text, glare), drop specks
    let mask: Vec<bool> = m.iter().map(|v| *v > 0.5).collect();
    let (mut q, score) = quad_from_mask(&mask, w, h)?;
    for p in q.iter_mut() {
        *p = [p[0] / k, p[1] / k];
    }
    Some((q, score))
}

/// Same, but the foreground mask comes from an AI matte (any size, 0..255).
pub fn detect_from_matte(matte: &GrayImage, img_w: u32, img_h: u32) -> Option<([Pt; 4], f32)> {
    let (w, h) = (matte.width() as usize, matte.height() as usize);
    let mask: Vec<bool> = matte.pixels().map(|p| p[0] > 127).collect();
    let (mut q, score) = quad_from_mask(&mask, w, h)?;
    let (kx, ky) = (img_w as f32 / w as f32, img_h as f32 / h as f32);
    for p in q.iter_mut() {
        *p = [p[0] * kx, p[1] * ky];
    }
    Some((q, score))
}

/// Inset default when nothing is found: nearly the whole picture.
pub fn full_quad(w: u32, h: u32) -> [Pt; 4] {
    let (mx, my) = (w as f32 * 0.04, h as f32 * 0.04);
    [[mx, my], [w as f32 - mx, my], [w as f32 - mx, h as f32 - my], [mx, h as f32 - my]]
}

// ------------------------------------------------------------------ clean-up
/// Paper-document shadow / uneven-light removal: estimate the paper colour with a big max-filter
/// (ignores thin text), smooth it, and divide it out so the paper becomes even white.
pub fn remove_shadows(f: &mut Fimg, strength: f32) {
    let (w, h) = (f.w, f.h);
    let k = (w.max(h) as f32 / 220.0).max(1.0);
    let (sw, sh) = (((w as f32 / k) as usize).max(4), ((h as f32 / k) as usize).max(4));
    // small luma-per-channel copy (box downscale)
    let mut small = vec![[0f32; 3]; sw * sh];
    for y in 0..sh {
        for x in 0..sw {
            let (x0, x1) = ((x as f32 * k) as usize, (((x + 1) as f32 * k) as usize).min(w).max((x as f32 * k) as usize + 1));
            let (y0, y1) = ((y as f32 * k) as usize, (((y + 1) as f32 * k) as usize).min(h).max((y as f32 * k) as usize + 1));
            let mut acc = [0f32; 3];
            let mut n = 0f32;
            for yy in y0..y1.min(h) {
                for xx in x0..x1.min(w) {
                    let p = f.d[yy * w + xx];
                    for c in 0..3 {
                        acc[c] += p[c];
                    }
                    n += 1.0;
                }
            }
            small[y * sw + x] = [acc[0] / n.max(1.0), acc[1] / n.max(1.0), acc[2] / n.max(1.0)];
        }
    }
    // max filter (text is thin, so it disappears), then blur
    let r = ((sw.min(sh) as f32 * 0.06).round() as usize).max(2);
    let mut bg = vec![[0f32; 3]; sw * sh];
    for c in 0..3 {
        let plane: Vec<f32> = small.iter().map(|p| p[c]).collect();
        let mut tmp = vec![0f32; sw * sh];
        for y in 0..sh {
            for x in 0..sw {
                let (lo, hi) = (x.saturating_sub(r), (x + r).min(sw - 1));
                tmp[y * sw + x] = (lo..=hi).map(|i| plane[y * sw + i]).fold(0.0, f32::max);
            }
        }
        let mut mx = vec![0f32; sw * sh];
        for y in 0..sh {
            let (lo, hi) = (y.saturating_sub(r), (y + r).min(sh - 1));
            for x in 0..sw {
                mx[y * sw + x] = (lo..=hi).map(|j| tmp[j * sw + x]).fold(0.0, f32::max);
            }
        }
        let bl = gauss_blur(&mx, sw, sh, r as f32 * 0.8);
        for i in 0..sw * sh {
            bg[i][c] = bl[i].max(0.12);
        }
    }
    let s = strength.clamp(0.0, 1.0);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) / k - 0.5).clamp(0.0, sh as f32 - 1.001);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        for x in 0..w {
            let fx = ((x as f32 + 0.5) / k - 0.5).clamp(0.0, sw as f32 - 1.001);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let p = &mut f.d[y * w + x];
            for c in 0..3 {
                let b = |xx: usize, yy: usize| bg[yy * sw + xx][c];
                let top = b(x0, y0) + (b(x0 + 1, y0) - b(x0, y0)) * tx;
                let bot = b(x0, y0 + 1) + (b(x0 + 1, y0 + 1) - b(x0, y0 + 1)) * tx;
                let local = top + (bot - top) * ty;
                // paper = 0.97 white
                let v = (p[c] / local * 0.97).clamp(0.0, 1.0);
                p[c] = lerp(p[c], v, s);
            }
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct DocOpts {
    /// remove cast shadows / uneven paper colour (documents)
    pub shadow: f32,
    pub even_light: f32,
    pub auto_levels: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub colour: f32,
    pub sharpen: f32,
    pub denoise: f32,
    /// 0 = colour, 1 = grey, 2 = scan (white paper, dark text)
    pub mode: u8,
    /// remove red / blue / any other coloured ink (stamps, seals, pen writing): 0 = off .. 1 = strong
    pub rm_red: f32,
    pub rm_blue: f32,
    pub rm_pen: f32,
    /// hand-erased spots: x, y (0..1 of the page) and radius (fraction of page width)
    pub rm_dots: Vec<[f32; 3]>,
}
impl Default for DocOpts {
    fn default() -> Self {
        DocOpts { shadow: 0.0, even_light: 0.6, auto_levels: 0.7, brightness: 0.0, contrast: 0.08, colour: 0.05, sharpen: 0.45, denoise: 0.15, mode: 0, rm_red: 0.0, rm_blue: 0.0, rm_pen: 0.0, rm_dots: Vec::new() }
    }
}

/// Fill the masked pixels (mask 0..1) from the bright paper around them, so a stamp or writing disappears.
pub fn fill_from_paper(f: &mut Fimg, mask: &[f32]) {
    let (w, h) = (f.w, f.h);
    if mask.iter().all(|m| *m <= 0.001) {
        return;
    }
    let l = f.luma();
    let wgt: Vec<f32> = mask.iter().zip(&l).map(|(m, l)| (1.0 - m.min(1.0)) * smoothstep(0.45, 0.75, *l)).collect();
    let mut pending: Vec<bool> = mask.iter().map(|m| *m > 0.0).collect();
    let mut r = (w / 45).max(3);
    let src = f.d.clone();
    let planes: Vec<Vec<f32>> = (0..3).map(|c| src.iter().zip(&wgt).map(|(p, w)| p[c] * w).collect()).collect();
    for _ in 0..6 {
        if !pending.iter().any(|p| *p) {
            break;
        }
        let wb = box_blur(&wgt, w, h, r);
        let chans: Vec<Vec<f32>> = planes.iter().map(|pl| box_blur(pl, w, h, r)).collect();
        for i in 0..f.d.len() {
            if !pending[i] || wb[i] < 0.02 {
                continue;
            }
            let m = mask[i].min(1.0);
            for c in 0..3 {
                let bg = (chans[c][i] / wb[i]).clamp(0.0, 1.0);
                f.d[i][c] = src[i][c] + (bg - src[i][c]) * m;
            }
            pending[i] = false;
        }
        r = (r * 2).min(w.max(h) / 3);
    }
}

/// Remove coloured ink (stamps, pen) and hand-erased spots.
pub fn remove_marks(f: &mut Fimg, o: &DocOpts) {
    let (w, h) = (f.w, f.h);
    let mut mask = vec![0f32; w * h];
    if o.rm_red > 0.0 || o.rm_blue > 0.0 || o.rm_pen > 0.0 {
        for (i, p) in f.d.iter().enumerate() {
            let (r, g, b) = (p[0], p[1], p[2]);
            let sat = r.max(g).max(b) - r.min(g).min(b);
            let mut m = 0f32;
            if o.rm_red > 0.0 {
                let dom = (r - g.max(b)).min(r - (g + b) * 0.5);
                let t = lerp(0.20, 0.07, o.rm_red);
                m = m.max(smoothstep(t, t + 0.12, dom));
            }
            if o.rm_blue > 0.0 {
                let dom = (b - r.max(g)).min(b - (r + g) * 0.5);
                let t = lerp(0.14, 0.04, o.rm_blue);
                m = m.max(smoothstep(t, t + 0.10, dom));
            }
            if o.rm_pen > 0.0 {
                let t = lerp(0.30, 0.10, o.rm_pen);
                m = m.max(smoothstep(t, t + 0.12, sat));
            }
            mask[i] = m;
        }
        let grown = box_blur(&mask, w, h, 2);
        for (m, g) in mask.iter_mut().zip(&grown) {
            *m = m.max(*g * 2.0).min(1.0);
        }
    }
    for d in &o.rm_dots {
        let (cx, cy, r) = (d[0] * w as f32, d[1] * h as f32, (d[2] * w as f32).max(2.0));
        let x0 = (cx - r - 2.0).floor().max(0.0) as usize;
        let x1 = (cx + r + 2.0).ceil().min(w as f32 - 1.0) as usize;
        let y0 = (cy - r - 2.0).floor().max(0.0) as usize;
        let y1 = (cy + r + 2.0).ceil().min(h as f32 - 1.0) as usize;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dd = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                let v = 1.0 - smoothstep(r * 0.85, r + 1.5, dd);
                let m = &mut mask[y * w + x];
                *m = m.max(v);
            }
        }
    }
    fill_from_paper(f, &mask);
}

pub fn enhance_doc(img: &RgbImage, o: &DocOpts) -> RgbImage {
    let mut f = Fimg::from_rgb(img);
    let (w, h) = (f.w, f.h);
    remove_marks(&mut f, o);
    adjust::denoise(&mut f, o.denoise);
    if o.shadow > 0.0 {
        remove_shadows(&mut f, o.shadow);
    }
    // even out a lighting gradient across the card (very large blur only, so print/photo are untouched)
    if o.even_light > 0.0 {
        let l = f.luma();
        let sm = gauss_blur(&box_blur(&l, w, h, (w / 60).max(1)), w, h, (w as f32 / 5.0).max(4.0));
        let mean = sm.iter().sum::<f32>() / sm.len() as f32;
        for (i, p) in f.d.iter_mut().enumerate() {
            let g = lerp(1.0, (mean / sm[i].max(0.05)).clamp(0.7, 1.5), o.even_light);
            for c in 0..3 {
                p[c] = (p[c] * g).clamp(0.0, 1.0);
            }
        }
    }
    // stretch levels on luma percentiles
    if o.auto_levels > 0.0 {
        let mut hist = [0u32; 256];
        for p in &f.d {
            hist[(luma(*p) * 255.0) as usize & 255] += 1;
        }
        let total = f.d.len() as f32;
        let pct = |q: f32| {
            let mut acc = 0f32;
            for (b, c) in hist.iter().enumerate() {
                acc += *c as f32;
                if acc >= q * total {
                    return b as f32 / 255.0;
                }
            }
            1.0
        };
        let (lo, hi) = (pct(0.004), pct(0.996));
        if hi - lo > 0.2 {
            for p in f.d.iter_mut() {
                for c in 0..3 {
                    let v = ((p[c] - lo) / (hi - lo)).clamp(0.0, 1.0);
                    p[c] = lerp(p[c], v, o.auto_levels);
                }
            }
        }
    }
    adjust::tone(&mut f, o.brightness, o.contrast, o.colour, 0.0);
    if o.mode == 1 {
        adjust::to_grey(&mut f);
    } else if o.mode == 2 {
        adjust::to_grey(&mut f);
        let g = f.plane(0);
        let bg = box_blur(&g, w, h, (w / 25).max(3));
        let v: Vec<f32> = g.iter().zip(&bg).map(|(a, b)| smoothstep(0.55, 0.95, a / b.max(0.05))).collect();
        for (p, x) in f.d.iter_mut().zip(&v) {
            *p = [*x, *x, *x];
        }
    }
    adjust::sharpen(&mut f, o.sharpen);
    f.to_rgb()
}

// ------------------------------------------------------------------ page layout
/// One card / document placed on the page. Position and size are in millimetres.
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub struct PageItem {
    /// which picture slot it shows (0 = front, 1 = back)
    pub slot: usize,
    pub x_mm: f32,
    pub y_mm: f32,
    pub w_mm: f32,
    pub h_mm: f32,
    /// keep width / height ratio while resizing
    pub lock: bool,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct PageOpts {
    pub paper_idx: usize,
    pub custom_w_mm: f32,
    pub custom_h_mm: f32,
    /// auto arrange: front above back (true) or side by side (false)
    pub stack: bool,
    pub gap_mm: f32,
    pub margin_mm: f32,
    pub top_align: bool,
    pub outline: bool,
    pub dpi: u32,
    pub items: Vec<PageItem>,
}
impl Default for PageOpts {
    fn default() -> Self {
        PageOpts { paper_idx: 0, custom_w_mm: 210.0, custom_h_mm: 297.0, stack: true, gap_mm: 10.0, margin_mm: 10.0, top_align: false, outline: true, dpi: 300, items: vec![] }
    }
}
impl PageOpts {
    pub fn paper_mm(&self) -> (f32, f32) {
        if self.paper_idx == crate::sheet::CUSTOM_PAPER {
            (self.custom_w_mm.max(30.0), self.custom_h_mm.max(30.0))
        } else {
            let p = &crate::sheet::PAPERS[self.paper_idx.min(crate::sheet::PAPERS.len() - 1)];
            (p.w_mm, p.h_mm)
        }
    }
    pub fn mm_px(&self, mm: f32) -> u32 {
        (mm / 25.4 * self.dpi as f32).round().max(1.0) as u32
    }
    /// every item is completely inside the paper
    pub fn fits(&self) -> bool {
        let (pw, ph) = self.paper_mm();
        self.items.iter().all(|i| i.x_mm >= -0.3 && i.y_mm >= -0.3 && i.x_mm + i.w_mm <= pw + 0.3 && i.y_mm + i.h_mm <= ph + 0.3)
    }
    /// Arrange all items in a neat stack / row (shrinks them evenly if they do not fit).
    pub fn auto_arrange(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let (pw, ph) = self.paper_mm();
        let (m, gap) = (self.margin_mm, self.gap_mm);
        let n = self.items.len() as f32;
        let (mut tw, mut th) = (0f32, 0f32);
        for i in &self.items {
            if self.stack {
                tw = tw.max(i.w_mm);
                th += i.h_mm;
            } else {
                tw += i.w_mm;
                th = th.max(i.h_mm);
            }
        }
        if self.stack {
            th += gap * (n - 1.0);
        } else {
            tw += gap * (n - 1.0);
        }
        let (aw, ah) = ((pw - 2.0 * m).max(10.0), (ph - 2.0 * m).max(10.0));
        let k = (aw / tw).min(ah / th).min(1.0);
        for i in self.items.iter_mut() {
            i.w_mm *= k;
            i.h_mm *= k;
        }
        let (tw, th) = (tw * k, th * k);
        let (mut x, mut y) = ((pw - tw) / 2.0, if self.top_align { m } else { (ph - th) / 2.0 });
        let (top, gapk) = (self.top_align, gap * k);
        for i in self.items.iter_mut() {
            if self.stack {
                i.x_mm = (pw - i.w_mm) / 2.0;
                i.y_mm = y;
                y += i.h_mm + gapk;
            } else {
                i.x_mm = x;
                i.y_mm = if top { m } else { (ph - i.h_mm) / 2.0 };
                x += i.w_mm + gapk;
            }
        }
    }
    /// Biggest size that fits inside the margins with the given width/height ratio, centred.
    pub fn fit_item(&mut self, idx: usize, aspect: f32) {
        let (pw, ph) = self.paper_mm();
        let (aw, ah) = ((pw - 2.0 * self.margin_mm).max(10.0), (ph - 2.0 * self.margin_mm).max(10.0));
        let (w, h) = if aw / ah > aspect { (ah * aspect, ah) } else { (aw, aw / aspect) };
        if let Some(i) = self.items.get_mut(idx) {
            i.w_mm = w;
            i.h_mm = h;
            i.x_mm = (pw - w) / 2.0;
            i.y_mm = (ph - h) / 2.0;
        }
    }
}

/// Paste the already-rendered item pictures (`imgs[i]` is item i at its size and the page dpi) on a white page.
pub fn compose_items(o: &PageOpts, imgs: &[RgbImage]) -> RgbImage {
    let (pw_mm, ph_mm) = o.paper_mm();
    let (pw, ph) = (o.mm_px(pw_mm), o.mm_px(ph_mm));
    let mut page = RgbImage::from_pixel(pw, ph, Rgb([255, 255, 255]));
    let line = (o.dpi as f32 / 150.0).round().max(1.0) as i64;
    for (it, img) in o.items.iter().zip(imgs) {
        let (x, y) = ((it.x_mm / 25.4 * o.dpi as f32).round() as i64, (it.y_mm / 25.4 * o.dpi as f32).round() as i64);
        imageops::replace(&mut page, img, x, y);
        if o.outline {
            let (x0, y0) = (x - line, y - line);
            let (x1, y1) = (x + img.width() as i64 + line, y + img.height() as i64 + line);
            let mut put = |xx: i64, yy: i64| {
                if xx >= 0 && yy >= 0 && (xx as u32) < pw && (yy as u32) < ph {
                    page.put_pixel(xx as u32, yy as u32, Rgb([150, 150, 150]));
                }
            };
            for t in 0..line {
                for xx in x0..x1 {
                    put(xx, y0 + t);
                    put(xx, y1 - 1 - t);
                }
                for yy in y0..y1 {
                    put(x0 + t, yy);
                    put(x1 - 1 - t, yy);
                }
            }
        }
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake "card" picture with a coloured gradient and dark bars.
    fn card(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            if fy < 0.14 {
                Rgb([40, 100, 80]) // header close to the table colour
            } else if (fy > 0.3 && fy < 0.38 && fx > 0.3 && fx < 0.9) || (fy > 0.6 && fy < 0.66 && fx > 0.3 && fx < 0.7) {
                Rgb([30, 30, 40])
            } else if fx < 0.25 && fy > 0.2 && fy < 0.8 {
                Rgb([200, 160, 130])
            } else {
                Rgb([(120.0 + 90.0 * fx) as u8, (190.0 + 40.0 * fy) as u8, 225])
            }
        })
    }

    /// Paste the card into a bigger noisy picture, seen in perspective, return (picture, true quad).
    fn scene(q: [Pt; 4]) -> RgbImage {
        let c = card(800, 500);
        let mut bgimg = RgbImage::from_fn(1100, 850, |x, y| {
            let n = ((x * 7 + y * 13) % 11) as i32;
            Rgb([(105 + n) as u8, (95 + n) as u8, (80 + n) as u8])
        });
        let rect = [[0.0, 0.0], [800.0, 0.0], [800.0, 500.0], [0.0, 500.0]];
        let h = solve_homography(q, rect).unwrap();
        for y in 0..bgimg.height() {
            for x in 0..bgimg.width() {
                let (u, v) = apply(&h, x as f32 + 0.5, y as f32 + 0.5);
                if u >= 0.0 && v >= 0.0 && u < 799.0 && v < 499.0 {
                    bgimg.put_pixel(x, y, *c.get_pixel(u as u32, v as u32));
                }
            }
        }
        bgimg
    }

    #[test]
    fn homography_maps_corners() {
        let q = [[100.0, 80.0], [900.0, 120.0], [860.0, 700.0], [60.0, 640.0]];
        let rect = [[0.0, 0.0], [800.0, 0.0], [800.0, 500.0], [0.0, 500.0]];
        let h = solve_homography(rect, q).unwrap();
        for i in 0..4 {
            let (x, y) = apply(&h, rect[i][0], rect[i][1]);
            assert!((x - q[i][0]).abs() < 0.01 && (y - q[i][1]).abs() < 0.01);
        }
    }

    #[test]
    fn warp_recovers_the_card() {
        let q = [[180.0, 120.0], [930.0, 170.0], [880.0, 720.0], [120.0, 650.0]];
        let pic = scene(q);
        let out = warp_quad(&pic, q, 400, 250);
        let truth = imageops::resize(&card(800, 500), 400, 250, imageops::FilterType::Triangle);
        let err: f32 = out.pixels().zip(truth.pixels()).map(|(a, b)| (0..3).map(|c| (a[c] as f32 - b[c] as f32).abs()).sum::<f32>() / 3.0).sum::<f32>() / (400.0 * 250.0);
        assert!(err < 6.0, "mean error {err}");
    }

    #[test]
    fn detects_tilted_card_corners() {
        let q = [[180.0, 120.0], [930.0, 170.0], [880.0, 720.0], [120.0, 650.0]];
        let pic = scene(q);
        let (d, score) = detect_classic(&pic).expect("card found");
        assert!(score > 0.9, "score {score}");
        let d = order_quad(d);
        for i in 0..4 {
            assert!(dist(d[i], q[i]) < 14.0, "corner {i}: found {:?} true {:?}", d[i], q[i]);
        }
    }

    #[test]
    fn detects_straight_card() {
        let q = [[150.0, 200.0], [950.0, 200.0], [950.0, 700.0], [150.0, 700.0]];
        let (d, _) = detect_classic(&scene(q)).expect("found");
        let d = order_quad(d);
        for i in 0..4 {
            assert!(dist(d[i], q[i]) < 10.0, "{:?} vs {:?}", d[i], q[i]);
        }
    }

    #[test]
    fn plain_background_finds_nothing() {
        let img = RgbImage::from_pixel(600, 400, Rgb([120, 110, 100]));
        assert!(detect_classic(&img).is_none());
    }

    #[test]
    fn rotation_by_corner_shift() {
        let q = [[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]];
        let r = rotate_quad(q, 1);
        assert_eq!(r[0], q[1]);
        let (w, h) = quad_wh(&r);
        assert!(h > w);
        // a portrait-shot landscape card gets turned
        let p = [[0.0, 0.0], [5.0, 0.0], [5.0, 10.0], [0.0, 10.0]];
        assert_eq!(auto_rot(&p, true), 1);
        assert_eq!(auto_rot(&q, true), 0);
    }

    #[test]
    fn order_quad_sorts() {
        let q = [[10.0, 5.0], [0.0, 0.0], [0.0, 5.0], [10.0, 0.0]];
        let o = order_quad(q);
        assert_eq!(o, [[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]);
    }

    fn item(slot: usize, w: f32, h: f32) -> PageItem {
        PageItem { slot, x_mm: 0.0, y_mm: 0.0, w_mm: w, h_mm: h, lock: true }
    }

    #[test]
    fn two_cards_on_a4_at_real_size() {
        let mut o = PageOpts::default();
        o.items = vec![item(0, 85.6, 54.0), item(1, 85.6, 54.0)];
        o.auto_arrange();
        assert!(o.fits());
        assert!((o.items[0].w_mm - 85.6).abs() < 0.01); // not shrunk
        assert!(o.items[1].y_mm > o.items[0].y_mm + 54.0);
        let imgs: Vec<RgbImage> = o.items.iter().map(|i| RgbImage::from_pixel(o.mm_px(i.w_mm), o.mm_px(i.h_mm), Rgb([10, 90, 200]))).collect();
        let page = compose_items(&o, &imgs);
        assert_eq!((page.width(), page.height()), (2480, 3508));
        assert_ne!(page.get_pixel(1240, 1300).0, [255, 255, 255]);
        assert_ne!(page.get_pixel(1240, 2200).0, [255, 255, 255]);
        assert_eq!(page.get_pixel(30, 30).0, [255, 255, 255]);
    }

    #[test]
    fn items_can_be_moved_freely_and_detect_overflow() {
        let mut o = PageOpts::default();
        o.items = vec![item(0, 85.6, 54.0)];
        o.items[0].x_mm = 150.0; // sticks out to the right of A4 (210)
        assert!(!o.fits());
        o.items[0].x_mm = 100.0;
        assert!(o.fits());
    }

    #[test]
    fn oversized_cards_shrink_to_fit_when_arranged() {
        let mut o = PageOpts { paper_idx: 5, ..Default::default() }; // A6 105x148
        o.items = vec![item(0, 85.6, 54.0), item(1, 85.6, 54.0), item(0, 85.6, 54.0)];
        o.auto_arrange();
        assert!(o.fits());
        assert!(o.items[0].w_mm < 85.6);
    }

    #[test]
    fn fit_item_keeps_the_ratio() {
        let mut o = PageOpts::default();
        o.items = vec![item(0, 50.0, 50.0)];
        o.fit_item(0, 0.5); // tall document
        let i = &o.items[0];
        assert!((i.w_mm / i.h_mm - 0.5).abs() < 1e-3);
        assert!(o.fits());
        assert!((i.x_mm + i.w_mm / 2.0 - 105.0).abs() < 0.01);
    }

    #[test]
    fn shadow_removal_evens_a_shaded_page() {
        // white paper with a dark gradient shadow on the left and a thin black line of "text"
        let mut img = RgbImage::from_fn(300, 200, |x, _| {
            let v = (120.0 + 130.0 * x as f32 / 300.0) as u8;
            Rgb([v, v, v])
        });
        for x in 20..280 {
            img.put_pixel(x, 100, Rgb([10, 10, 10]));
        }
        let out = enhance_doc(&img, &DocOpts { shadow: 1.0, even_light: 0.0, auto_levels: 0.0, contrast: 0.0, colour: 0.0, sharpen: 0.0, denoise: 0.0, ..Default::default() });
        let (l, r) = (out.get_pixel(15, 50)[0] as i32, out.get_pixel(285, 50)[0] as i32);
        assert!((l - r).abs() < 25, "left {l} right {r}");
        assert!(out.get_pixel(150, 100)[0] < 120, "text must stay dark");
    }

    #[test]
    fn enhance_keeps_size_and_brightens_dull_card() {
        let dull = RgbImage::from_fn(200, 120, |x, _| Rgb([(90 + x / 2) as u8, 100, 110]));
        let out = enhance_doc(&dull, &DocOpts { even_light: 0.0, ..Default::default() });
        assert_eq!(out.dimensions(), (200, 120));
        let range = |i: &RgbImage| {
            let v: Vec<i32> = i.pixels().map(|p| p[0] as i32).collect();
            v.iter().max().unwrap() - v.iter().min().unwrap()
        };
        assert!(range(&out) > range(&dull), "{} vs {}", range(&out), range(&dull));
    }

    #[test]
    fn stamp_and_hand_erase_remove_marks() {
        let mut img = RgbImage::from_pixel(300, 200, Rgb([235, 232, 225]));
        for y in 40..70 {
            for x in 40..90 {
                img.put_pixel(x, y, Rgb([200, 40, 50])); // red stamp
            }
        }
        for y in 120..130 {
            for x in 150..240 {
                img.put_pixel(x, y, Rgb([20, 20, 25])); // black writing
            }
        }
        for y in 150..160 {
            for x in 20..60 {
                img.put_pixel(x, y, Rgb([20, 20, 25])); // black text that must stay
            }
        }
        let base = DocOpts { even_light: 0.0, auto_levels: 0.0, contrast: 0.0, colour: 0.0, sharpen: 0.0, denoise: 0.0, ..Default::default() };
        let out = enhance_doc(&img, &DocOpts { rm_red: 1.0, rm_dots: vec![[0.6, 0.625, 0.16]], ..base.clone() });
        assert!(out.get_pixel(65, 55)[1] > 200, "stamp still there {:?}", out.get_pixel(65, 55));
        assert!(out.get_pixel(195, 125)[0] > 200, "writing still there {:?}", out.get_pixel(195, 125));
        assert!(out.get_pixel(40, 155)[0] < 60, "kept text was removed");
        let keep = enhance_doc(&img, &base);
        assert!(keep.get_pixel(65, 55)[1] < 100);
    }
}
