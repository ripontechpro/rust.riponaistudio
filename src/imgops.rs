//! Small, dependency-free image maths: float image, box / gaussian blur, guided filter,
//! rotated crop sampling. Everything here is pure CPU Rust.
use image::{imageops, GrayImage, RgbImage};

/// Float RGB image, values 0..1.
#[derive(Clone)]
pub struct Fimg {
    pub w: usize,
    pub h: usize,
    pub d: Vec<[f32; 3]>,
}

impl Fimg {
    pub fn from_rgb(im: &RgbImage) -> Fimg {
        let (w, h) = (im.width() as usize, im.height() as usize);
        let d = im
            .pixels()
            .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0])
            .collect();
        Fimg { w, h, d }
    }
    pub fn to_rgb(&self) -> RgbImage {
        let mut out = RgbImage::new(self.w as u32, self.h as u32);
        for (i, p) in out.pixels_mut().enumerate() {
            let s = self.d[i];
            p.0 = [q8(s[0]), q8(s[1]), q8(s[2])];
        }
        out
    }
    pub fn plane(&self, c: usize) -> Vec<f32> {
        self.d.iter().map(|p| p[c]).collect()
    }
    pub fn set_plane(&mut self, c: usize, v: &[f32]) {
        for (p, x) in self.d.iter_mut().zip(v) {
            p[c] = *x;
        }
    }
    pub fn luma(&self) -> Vec<f32> {
        self.d.iter().map(|p| luma(*p)).collect()
    }
}

#[inline]
pub fn q8(v: f32) -> u8 {
    (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}
#[inline]
pub fn luma(p: [f32; 3]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Mean filter with a (2r+1) window, edges use the clamped window (exact mean).
pub fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 || w == 0 || h == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0f32; w * h];
    let mut pre = vec![0f64; w + 1];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        for x in 0..w {
            pre[x + 1] = pre[x] + row[x] as f64;
        }
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            tmp[y * w + x] = ((pre[hi + 1] - pre[lo]) / (hi - lo + 1) as f64) as f32;
        }
    }
    let mut pre = vec![0f64; (h + 1) * w];
    for y in 0..h {
        for x in 0..w {
            pre[(y + 1) * w + x] = pre[y * w + x] + tmp[y * w + x] as f64;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let lo = y.saturating_sub(r);
        let hi = (y + r).min(h - 1);
        let n = (hi - lo + 1) as f64;
        for x in 0..w {
            out[y * w + x] = ((pre[(hi + 1) * w + x] - pre[lo * w + x]) / n) as f32;
        }
    }
    out
}

/// Gaussian-like blur (3 box passes).
pub fn gauss_blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma < 0.3 {
        return src.to_vec();
    }
    let wi = ((12.0 * sigma * sigma / 3.0 + 1.0).sqrt()).floor().max(1.0);
    let r = (((wi - 1.0) / 2.0).round() as usize).max(1);
    let a = box_blur(src, w, h, r);
    let b = box_blur(&a, w, h, r);
    box_blur(&b, w, h, r)
}

/// Guided filter (He et al.) – edge-aware smoothing / matte refinement.
pub fn guided(i: &[f32], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let n = w * h;
    let mean_i = box_blur(i, w, h, r);
    let mean_p = box_blur(p, w, h, r);
    let ip: Vec<f32> = (0..n).map(|k| i[k] * p[k]).collect();
    let ii: Vec<f32> = (0..n).map(|k| i[k] * i[k]).collect();
    let corr_ip = box_blur(&ip, w, h, r);
    let corr_ii = box_blur(&ii, w, h, r);
    let mut a = vec![0f32; n];
    let mut b = vec![0f32; n];
    for k in 0..n {
        let var = corr_ii[k] - mean_i[k] * mean_i[k];
        let cov = corr_ip[k] - mean_i[k] * mean_p[k];
        a[k] = cov / (var + eps);
        b[k] = mean_p[k] - a[k] * mean_i[k];
    }
    let ma = box_blur(&a, w, h, r);
    let mb = box_blur(&b, w, h, r);
    (0..n).map(|k| ma[k] * i[k] + mb[k]).collect()
}

fn bilinear(data: &[u8], sw: usize, sh: usize, ch: usize, x: f32, y: f32, out: &mut [u8]) {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let xi = x0 as isize;
    let yi = y0 as isize;
    let cl = |v: isize, m: usize| v.clamp(0, m as isize - 1) as usize;
    let (xa, xb, ya, yb) = (cl(xi, sw), cl(xi + 1, sw), cl(yi, sh), cl(yi + 1, sh));
    for c in 0..ch {
        let p00 = data[(ya * sw + xa) * ch + c] as f32;
        let p10 = data[(ya * sw + xb) * ch + c] as f32;
        let p01 = data[(yb * sw + xa) * ch + c] as f32;
        let p11 = data[(yb * sw + xb) * ch + c] as f32;
        let top = p00 + (p10 - p00) * fx;
        let bot = p01 + (p11 - p01) * fx;
        out[c] = (top + (bot - top) * fy + 0.5) as u8;
    }
}

/// Sample a rotated rectangle (centre cx,cy ; size cw x ch ; angle in degrees) of a
/// `chn`-channel 8-bit buffer into an `ow x oh` buffer. Also returns a coverage mask
/// (255 = inside the source picture, 0 = outside).
pub fn sample_rect(
    data: &[u8], sw: usize, sh: usize, chn: usize,
    cx: f32, cy: f32, cw: f32, chh: f32, angle_deg: f32,
    ow: usize, oh: usize,
) -> (Vec<u8>, Vec<u8>) {
    let (sn, cs) = angle_deg.to_radians().sin_cos();
    let mut out = vec![0u8; ow * oh * chn];
    let mut cov = vec![0u8; ow * oh];
    for v in 0..oh {
        for u in 0..ow {
            let lx = ((u as f32 + 0.5) / ow as f32 - 0.5) * cw;
            let ly = ((v as f32 + 0.5) / oh as f32 - 0.5) * chh;
            let sx = cx + lx * cs - ly * sn - 0.5;
            let sy = cy + lx * sn + ly * cs - 0.5;
            let inside = sx >= -0.5 && sy >= -0.5 && sx <= sw as f32 - 0.5 && sy <= sh as f32 - 0.5;
            if inside {
                cov[v * ow + u] = 255;
                bilinear(data, sw, sh, chn, sx, sy, &mut out[(v * ow + u) * chn..(v * ow + u + 1) * chn]);
            }
        }
    }
    (out, cov)
}

/// High quality crop of a rotated rectangle from an RGB picture to exactly `ow x oh`.
/// Returns (rgb, coverage). Big pictures are pre-shrunk so the result stays sharp, not aliased.
pub fn crop_rgb(src: &RgbImage, cx: f32, cy: f32, cw: f32, ch: f32, angle: f32, ow: u32, oh: u32) -> (RgbImage, GrayImage) {
    let ratio = cw / ow as f32;
    let (mut cx, mut cy, mut cw, mut ch) = (cx, cy, cw, ch);
    let mut owned: Option<RgbImage> = None;
    if ratio > 2.2 {
        // crop the bounding box of the rotated rect, shrink it
        let (sn, cs) = angle.to_radians().sin_cos();
        let ex = (cw * cs.abs() + ch * sn.abs()) / 2.0 + 2.0;
        let ey = (cw * sn.abs() + ch * cs.abs()) / 2.0 + 2.0;
        let x0 = (cx - ex).floor().max(0.0);
        let y0 = (cy - ey).floor().max(0.0);
        let x1 = (cx + ex).ceil().min(src.width() as f32);
        let y1 = (cy + ey).ceil().min(src.height() as f32);
        if x1 - x0 > 4.0 && y1 - y0 > 4.0 {
            let sub = imageops::crop_imm(src, x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32).to_image();
            let k = 2.2 / ratio;
            let nw = ((sub.width() as f32 * k).round() as u32).max(2);
            let nh = ((sub.height() as f32 * k).round() as u32).max(2);
            let small = imageops::resize(&sub, nw, nh, imageops::FilterType::Lanczos3);
            let kx = nw as f32 / sub.width() as f32;
            let ky = nh as f32 / sub.height() as f32;
            cx = (cx - x0) * kx;
            cy = (cy - y0) * ky;
            cw *= kx;
            ch *= ky;
            owned = Some(small);
        }
    }
    let base: &RgbImage = owned.as_ref().unwrap_or(src);
    let ss = if cw / ow as f32 > 1.0 { 2 } else { 1 };
    let (bw, bh) = ((ow * ss) as usize, (oh * ss) as usize);
    let (buf, cov) = sample_rect(base.as_raw(), base.width() as usize, base.height() as usize, 3, cx, cy, cw, ch, angle, bw, bh);
    let mut img = RgbImage::from_raw(bw as u32, bh as u32, buf).unwrap();
    let mut cv = GrayImage::from_raw(bw as u32, bh as u32, cov).unwrap();
    if ss > 1 {
        img = imageops::resize(&img, ow, oh, imageops::FilterType::Lanczos3);
        cv = imageops::resize(&cv, ow, oh, imageops::FilterType::Triangle);
    }
    (img, cv)
}

/// Same geometry for the (lower resolution) alpha matte; coordinates are in SOURCE pixels.
pub fn crop_gray(alpha: &GrayImage, src_w: u32, cx: f32, cy: f32, cw: f32, ch: f32, angle: f32, ow: u32, oh: u32) -> GrayImage {
    let k = alpha.width() as f32 / src_w as f32;
    let (buf, _) = sample_rect(alpha.as_raw(), alpha.width() as usize, alpha.height() as usize, 1, cx * k, cy * k, cw * k, ch * k, angle, ow as usize, oh as usize);
    GrayImage::from_raw(ow, oh, buf).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn box_blur_keeps_constant() {
        let v = vec![0.5f32; 40 * 30];
        let o = box_blur(&v, 40, 30, 4);
        assert!(o.iter().all(|x| (x - 0.5).abs() < 1e-5));
    }
    #[test]
    fn crop_outside_picture_is_marked() {
        let src = RgbImage::from_pixel(100, 100, image::Rgb([10, 20, 30]));
        let (_, cov) = crop_rgb(&src, 0.0, 50.0, 100.0, 100.0, 0.0, 50, 50);
        assert_eq!(cov.get_pixel(2, 25)[0], 0); // left half is outside
        assert_eq!(cov.get_pixel(45, 25)[0], 255);
    }
}
