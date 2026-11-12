//! The whole photo pipeline: crop -> background -> enhance -> retouch -> dress -> caption.
//! `render_photo` is a pure function of (source, matte, Edit), so preview == export.
use crate::{adjust, head, imgops::*, text};
use image::{GrayImage, RgbImage, RgbaImage};
use serde::{Deserialize, Serialize};

pub struct Preset {
    pub name: &'static str,
    pub w_mm: f32,
    pub h_mm: f32,
    /// crown-to-chin as a fraction of photo height
    pub head_ratio: f32,
    /// space above the crown as a fraction of photo height
    pub top_margin: f32,
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "Passport 35 x 45 mm", w_mm: 35.0, h_mm: 45.0, head_ratio: 0.74, top_margin: 0.075 },
    Preset { name: "Passport 40 x 50 mm", w_mm: 40.0, h_mm: 50.0, head_ratio: 0.72, top_margin: 0.08 },
    Preset { name: "Passport 33 x 48 mm", w_mm: 33.0, h_mm: 48.0, head_ratio: 0.72, top_margin: 0.08 },
    Preset { name: "Visa 2 x 2 inch (51 x 51 mm)", w_mm: 50.8, h_mm: 50.8, head_ratio: 0.60, top_margin: 0.11 },
    Preset { name: "Stamp 20 x 25 mm", w_mm: 20.0, h_mm: 25.0, head_ratio: 0.72, top_margin: 0.08 },
    Preset { name: "Stamp 25 x 30 mm", w_mm: 25.0, h_mm: 30.0, head_ratio: 0.72, top_margin: 0.08 },
    Preset { name: "Bank 1.5 x 1.9 inch", w_mm: 38.1, h_mm: 48.26, head_ratio: 0.72, top_margin: 0.08 },
    Preset { name: "Custom size (mm)", w_mm: 35.0, h_mm: 45.0, head_ratio: 0.72, top_margin: 0.08 },
];
pub const CUSTOM_IDX: usize = 7;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug)]
pub struct Spot {
    pub x: f32, // 0..1 of photo width
    pub y: f32, // 0..1 of photo height
    pub r: f32, // fraction of photo width
}

/// One brush stroke on the background matte (source-pixel coordinates).
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub struct BrushStroke {
    /// false = restore the person (red), true = erase background (blue)
    pub erase: bool,
    /// radius in SOURCE pixels
    pub r: f32,
    pub pts: Vec<(f32, f32)>,
    /// 1 = hard edge, 0 = very soft (feathered) edge
    #[serde(default = "one")]
    pub hard: f32,
    /// 0..1 how strongly the brush works (1 = fully removes / restores)
    #[serde(default = "one")]
    pub strength: f32,
}
fn one() -> f32 {
    1.0
}

/// What a pen / lasso shape does to the background matte.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug)]
pub enum CutMode {
    /// remove the background inside the shape
    Erase,
    /// bring the person back inside the shape
    Restore,
    /// keep only what is inside the shape (everything outside is removed)
    KeepInside,
}

/// A closed shape drawn with the Pen (click points) or Lasso (drag) tool, in SOURCE pixels.
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub struct PolyCut {
    pub mode: CutMode,
    pub pts: Vec<(f32, f32)>,
    /// soft edge width in SOURCE pixels (0 = sharp)
    pub feather: f32,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    pub size_idx: usize,
    pub custom_w_mm: f32,
    pub custom_h_mm: f32,
    pub dpi: u32,
    // crop (source pixels)
    pub cx: f32,
    pub cy: f32,
    pub cw: f32,
    pub angle: f32,
    /// mirror the cropped photo left-right / top-bottom
    pub flip_h: bool,
    pub flip_v: bool,
    // background
    pub bg: [u8; 3],
    pub edge_refine: f32,
    pub edge_cut: f32,
    // enhance
    pub auto_enhance: bool,
    pub enhance_strength: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub shadows: f32,
    pub sharpen: f32,
    pub denoise: f32,
    pub bw: bool,
    // matte brush
    pub strokes: Vec<BrushStroke>,
    pub cuts: Vec<PolyCut>,
    // duet (two photos in one frame, e.g. bank photo 1.9 x 1.5 in)
    pub duet: bool,
    pub duet_w_mm: f32,
    pub duet_h_mm: f32,
    pub duet_between_mm: f32,
    pub duet_stack: bool,
    /// false = automatic layout (side by side / stacked); true = each photo has its own rectangle
    pub duet_free: bool,
    /// per photo (A, B): x, y, w, h as fractions of the frame - used when `duet_free`
    pub duet_rect: [[f32; 4]; 2],
    /// which photo (0 = A, 1 = B) this edit renders
    pub duet_active: u8,
    // retouch
    pub skin: f32,
    pub spots: Vec<Spot>,
    // dress
    pub dress_id: u32,
    pub dress_scale: f32,
    pub dress_x: f32,
    pub dress_y: f32,
    // caption
    pub caption: String,
    pub caption_date: String,
}

impl Default for Edit {
    fn default() -> Self {
        Edit {
            size_idx: 0,
            custom_w_mm: 35.0,
            custom_h_mm: 45.0,
            dpi: 300,
            cx: 0.0,
            cy: 0.0,
            cw: 100.0,
            angle: 0.0,
            flip_h: false,
            flip_v: false,
            bg: [255, 255, 255],
            edge_refine: 0.6,
            edge_cut: 0.12,
            auto_enhance: true,
            enhance_strength: 0.7,
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            shadows: 0.0,
            sharpen: 0.35,
            denoise: 0.0,
            bw: false,
            strokes: Vec::new(),
            cuts: Vec::new(),
            duet: false,
            duet_w_mm: 48.26,
            duet_h_mm: 38.1,
            duet_between_mm: 0.0,
            duet_stack: false,
            duet_free: false,
            duet_rect: [[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]],
            duet_active: 0,
            skin: 0.0,
            spots: Vec::new(),
            dress_id: 0,
            dress_scale: 1.0,
            dress_x: 0.0,
            dress_y: 0.0,
            caption: String::new(),
            caption_date: String::new(),
        }
    }
}

impl Edit {
    /// Swap everything that belongs to ONE photo (crop, colours, background brush, retouch);
    /// frame-wide things (size, dpi, duet layout, caption) stay where they are.
    pub fn swap_photo(&mut self, o: &mut Edit) {
        use std::mem::swap;
        swap(&mut self.cx, &mut o.cx);
        swap(&mut self.cy, &mut o.cy);
        swap(&mut self.cw, &mut o.cw);
        swap(&mut self.angle, &mut o.angle);
        swap(&mut self.flip_h, &mut o.flip_h);
        swap(&mut self.flip_v, &mut o.flip_v);
        swap(&mut self.bg, &mut o.bg);
        swap(&mut self.edge_refine, &mut o.edge_refine);
        swap(&mut self.edge_cut, &mut o.edge_cut);
        swap(&mut self.auto_enhance, &mut o.auto_enhance);
        swap(&mut self.enhance_strength, &mut o.enhance_strength);
        swap(&mut self.brightness, &mut o.brightness);
        swap(&mut self.contrast, &mut o.contrast);
        swap(&mut self.saturation, &mut o.saturation);
        swap(&mut self.shadows, &mut o.shadows);
        swap(&mut self.sharpen, &mut o.sharpen);
        swap(&mut self.denoise, &mut o.denoise);
        swap(&mut self.bw, &mut o.bw);
        swap(&mut self.strokes, &mut o.strokes);
        swap(&mut self.cuts, &mut o.cuts);
        swap(&mut self.skin, &mut o.skin);
        swap(&mut self.spots, &mut o.spots);
    }
    pub fn preset(&self) -> &'static Preset {
        &PRESETS[self.size_idx.min(PRESETS.len() - 1)]
    }
    /// (head height / photo height, space above crown / photo height)
    pub fn head_params(&self) -> (f32, f32) {
        if self.duet {
            (0.72, 0.08)
        } else {
            (self.preset().head_ratio, self.preset().top_margin)
        }
    }
    fn mm_to_px(&self, mm: f32) -> u32 {
        (mm / 25.4 * self.dpi as f32).round() as u32
    }
    /// Whole duet frame in pixels.
    pub fn duet_px(&self) -> (u32, u32) {
        (self.mm_to_px(self.duet_w_mm.clamp(15.0, 300.0)).max(16), self.mm_to_px(self.duet_h_mm.clamp(15.0, 300.0)).max(16))
    }
    pub fn duet_between_px(&self) -> u32 {
        self.mm_to_px(self.duet_between_mm.clamp(0.0, 40.0))
    }
    /// Automatic layout cells (x, y, w, h in px) for photo A and B.
    pub fn duet_auto_cells(&self) -> [(i32, i32, u32, u32); 2] {
        let (tw, th) = self.duet_px();
        let b = self.duet_between_px();
        if self.duet_stack {
            let ph = (th.saturating_sub(b) / 2).max(8);
            let y0 = (th.saturating_sub(2 * ph + b) / 2) as i32;
            [(0, y0, tw, ph), (0, y0 + (ph + b) as i32, tw, ph)]
        } else {
            let pw = (tw.saturating_sub(b) / 2).max(8);
            let x0 = (tw.saturating_sub(2 * pw + b) / 2) as i32;
            [(x0, 0, pw, th), (x0 + (pw + b) as i32, 0, pw, th)]
        }
    }
    /// Cells (x, y, w, h in px) where photo A and B are drawn inside the duet frame.
    pub fn duet_cells(&self) -> [(i32, i32, u32, u32); 2] {
        if !self.duet_free {
            return self.duet_auto_cells();
        }
        let (tw, th) = self.duet_px();
        let f = |r: [f32; 4]| ((r[0] * tw as f32).round() as i32, (r[1] * th as f32).round() as i32, ((r[2] * tw as f32).round() as u32).max(8), ((r[3] * th as f32).round() as u32).max(8));
        [f(self.duet_rect[0]), f(self.duet_rect[1])]
    }
    /// Start a free layout from the current automatic one.
    pub fn duet_make_free(&mut self) {
        if self.duet_free {
            return;
        }
        let (tw, th) = self.duet_px();
        let c = self.duet_auto_cells();
        for i in 0..2 {
            self.duet_rect[i] = [c[i].0 as f32 / tw as f32, c[i].1 as f32 / th as f32, c[i].2 as f32 / tw as f32, c[i].3 as f32 / th as f32];
        }
        self.duet_free = true;
    }
    pub fn size_mm(&self) -> (f32, f32) {
        if self.duet && self.duet_free {
            let c = self.duet_cells()[(self.duet_active as usize).min(1)];
            return (c.2 as f32 / self.dpi as f32 * 25.4, c.3 as f32 / self.dpi as f32 * 25.4);
        }
        if self.duet {
            let (w, h) = (self.duet_w_mm.clamp(15.0, 300.0), self.duet_h_mm.clamp(15.0, 300.0));
            let b = self.duet_between_mm.clamp(0.0, 40.0);
            return if self.duet_stack { (w, ((h - b) / 2.0).max(5.0)) } else { (((w - b) / 2.0).max(5.0), h) };
        }
        if self.size_idx == CUSTOM_IDX {
            (self.custom_w_mm.clamp(10.0, 300.0), self.custom_h_mm.clamp(10.0, 300.0))
        } else {
            (self.preset().w_mm, self.preset().h_mm)
        }
    }
    pub fn photo_px(&self) -> (u32, u32) {
        if self.duet {
            let c = self.duet_cells()[(self.duet_active as usize).min(1)];
            return (c.2, c.3);
        }
        let (w, h) = self.size_mm();
        let f = |mm: f32| ((mm / 25.4 * self.dpi as f32).round() as u32).max(8);
        (f(w), f(h))
    }
    pub fn aspect(&self) -> f32 {
        let (w, h) = self.photo_px();
        w as f32 / h as f32
    }
    pub fn crop_h(&self) -> f32 {
        self.cw / self.aspect()
    }
    /// turn the picture by 90 degrees steps (+1 = right, -1 = left); the crop frame stays upright
    pub fn rotate_quarter(&mut self, dir: i32) {
        let mut a = self.angle - 90.0 * dir as f32; // the frame turns the other way round the picture
        while a > 180.0 {
            a -= 360.0;
        }
        while a <= -180.0 {
            a += 360.0;
        }
        self.angle = a;
    }
    /// Centred crop that covers as much of the picture as the aspect allows.
    pub fn reset_crop(&mut self, iw: u32, ih: u32) {
        let a = self.aspect();
        let w = (iw as f32).min(ih as f32 * a);
        self.cw = w;
        self.cx = iw as f32 / 2.0;
        self.cy = ih as f32 / 2.0;
        self.angle = 0.0;
    }
    /// Place the crop so the head matches the preset (crown margin, head size, centred).
    pub fn auto_crop(&mut self, alpha: &GrayImage, src_w: u32, src_h: u32) -> bool {
        let Some(hd) = head::estimate(alpha) else { return false };
        let f = src_w as f32 / alpha.width() as f32;
        let (head_ratio, top_margin) = self.head_params();
        let head_h = (hd.chin_y - hd.crown_y) * f;
        let crop_h = head_h / head_ratio;
        self.cw = crop_h * self.aspect();
        self.cx = hd.cx * f;
        self.cy = hd.crown_y * f - top_margin * crop_h + crop_h / 2.0;
        self.angle = 0.0;
        let _ = src_h;
        true
    }
}

/// Render the photo (no caption) at exact output size.
pub fn render_photo(src: &RgbImage, alpha: Option<&GrayImage>, e: &Edit, dress: Option<&RgbaImage>) -> RgbImage {
    // brush strokes are applied to a copy of the matte (a full-opaque matte if no AI matte exists yet)
    let edited: GrayImage;
    let alpha = if e.strokes.is_empty() && e.cuts.is_empty() {
        alpha
    } else {
        let mut a = match alpha {
            Some(a) => a.clone(),
            None => {
                let k = (1536.0 / src.width().max(src.height()) as f32).min(1.0);
                GrayImage::from_pixel(((src.width() as f32 * k) as u32).max(8), ((src.height() as f32 * k) as u32).max(8), image::Luma([255]))
            }
        };
        apply_strokes(&mut a, src.width(), &e.strokes);
        apply_cuts(&mut a, src.width(), &e.cuts);
        edited = a;
        Some(&edited)
    };
    let (pw, ph) = e.photo_px();
    let (w, h) = (pw as usize, ph as usize);
    let n = w * h;
    let ch = e.crop_h();
    let (mut crop, mut cov) = crop_rgb(src, e.cx, e.cy, e.cw, ch, e.angle, pw, ph);
    if e.flip_h {
        crop = image::imageops::flip_horizontal(&crop);
        cov = image::imageops::flip_horizontal(&cov);
    }
    if e.flip_v {
        crop = image::imageops::flip_vertical(&crop);
        cov = image::imageops::flip_vertical(&cov);
    }
    let covf: Vec<f32> = cov.pixels().map(|p| p[0] as f32 / 255.0).collect();
    let mut img = Fimg::from_rgb(&crop);

    // ---------- matte ----------
    let bg = [e.bg[0] as f32 / 255.0, e.bg[1] as f32 / 255.0, e.bg[2] as f32 / 255.0];
    let mut a: Vec<f32>;
    if let Some(al) = alpha {
        let mut g = crop_gray(al, src.width(), e.cx, e.cy, e.cw, ch, e.angle, pw, ph);
        if e.flip_h {
            g = image::imageops::flip_horizontal(&g);
        }
        if e.flip_v {
            g = image::imageops::flip_vertical(&g);
        }
        let raw: Vec<f32> = g.pixels().map(|p| p[0] as f32 / 255.0).collect();
        let guide = img.luma();
        let r = ((w as f32 / 140.0).round() as usize).clamp(2, 8);
        let refined = guided(&guide, &raw, w, h, r, 4e-4);
        let t = e.edge_refine.clamp(0.0, 1.0);
        let lo = e.edge_cut.clamp(0.0, 0.45);
        a = (0..n).map(|i| smoothstep(lo, 1.0 - lo * 0.5, lerp(raw[i], refined[i].clamp(0.0, 1.0), t))).collect();
        // remove old-background colour spill on the soft edge
        let (mut s, mut cnt) = ([0f64; 3], 0f64);
        for i in 0..n {
            if raw[i] < 0.04 && covf[i] > 0.99 {
                for c in 0..3 {
                    s[c] += img.d[i][c] as f64;
                }
                cnt += 1.0;
            }
        }
        if cnt > 150.0 {
            let bo = [(s[0] / cnt) as f32, (s[1] / cnt) as f32, (s[2] / cnt) as f32];
            for i in 0..n {
                if a[i] > 0.02 && a[i] < 0.98 {
                    let k = a[i].max(0.4);
                    for c in 0..3 {
                        let f = ((img.d[i][c] - (1.0 - a[i]) * bo[c]) / k).clamp(0.0, 1.0);
                        img.d[i][c] = lerp(img.d[i][c], f, 0.8);
                    }
                }
            }
        }
    } else {
        a = vec![1.0; n];
    }
    for i in 0..n {
        a[i] *= covf[i];
    }
    for i in 0..n {
        for c in 0..3 {
            img.d[i][c] = img.d[i][c] * a[i] + bg[c] * (1.0 - a[i]);
        }
    }
    let person: Vec<f32> = if alpha.is_some() { a.clone() } else { covf.clone() };

    // ---------- heal spots ----------
    for s in &e.spots {
        adjust::heal(&mut img, s.x * w as f32, s.y * h as f32, (s.r * w as f32).max(2.0));
    }
    // ---------- enhance / retouch ----------
    if e.auto_enhance {
        adjust::auto_enhance(&mut img, &person, e.enhance_strength);
    }
    adjust::denoise(&mut img, e.denoise);
    adjust::skin_smooth(&mut img, &person, e.skin);
    // ---------- dress ----------
    if let Some(d) = dress {
        overlay_dress(&mut img, d, e);
    }
    adjust::tone(&mut img, e.brightness, e.contrast, e.saturation, e.shadows);
    if e.bw {
        adjust::to_grey(&mut img);
    }
    adjust::sharpen(&mut img, e.sharpen);
    img.to_rgb()
}

fn overlay_dress(img: &mut Fimg, d: &RgbaImage, e: &Edit) {
    let (w, h) = (img.w as f32, img.h as f32);
    let dw = (w * e.dress_scale).max(8.0);
    let dh = dw * d.height() as f32 / d.width() as f32;
    let small = image::imageops::resize(d, dw as u32, dh.max(8.0) as u32, image::imageops::FilterType::Lanczos3);
    let x0 = (w / 2.0 + e.dress_x * w - dw / 2.0).round() as i32;
    let y0 = (h + e.dress_y * h - dh).round() as i32;
    for (sx, sy, p) in small.enumerate_pixels() {
        let (x, y) = (x0 + sx as i32, y0 + sy as i32);
        if x < 0 || y < 0 || x >= img.w as i32 || y >= img.h as i32 {
            continue;
        }
        let al = p[3] as f32 / 255.0;
        if al <= 0.0 {
            continue;
        }
        let o = &mut img.d[y as usize * img.w + x as usize];
        for c in 0..3 {
            o[c] = o[c] * (1.0 - al) + (p[c] as f32 / 255.0) * al;
        }
    }
}

/// Adds a white strip with name / date under the photo.
pub fn add_caption(photo: &RgbImage, e: &Edit) -> RgbImage {
    let name = e.caption.trim();
    let date = e.caption_date.trim();
    if name.is_empty() && date.is_empty() {
        return photo.clone();
    }
    let lines = (!name.is_empty()) as u32 + (!date.is_empty()) as u32;
    let (w, h) = photo.dimensions();
    let strip = (h as f32 * 0.075 * lines as f32 + h as f32 * 0.02).round() as u32;
    let mut out = RgbImage::from_pixel(w, h + strip, image::Rgb([255, 255, 255]));
    image::imageops::replace(&mut out, photo, 0, 0);
    let lh = strip as f32 / lines as f32;
    let mut y = h as f32 + lh * 0.5;
    for t in [name, date] {
        if !t.is_empty() {
            text::draw_centered(&mut out, t, w as f32 / 2.0, y, lh * 0.62, w as f32 * 0.94, [20, 20, 20]);
            y += lh;
        }
    }
    out
}

/// Paint restore / erase strokes into the matte (alpha px = source px * k).
/// Each stroke is first collected as a coverage map (so overlapping dabs never pile up) and applied once,
/// with its own softness (`hard`) and strength.
pub fn apply_strokes(alpha: &mut GrayImage, src_w: u32, strokes: &[BrushStroke]) {
    let k = alpha.width() as f32 / src_w as f32;
    let (aw, ah) = (alpha.width() as i32, alpha.height() as i32);
    for s in strokes {
        if s.pts.is_empty() {
            continue;
        }
        let r = (s.r * k).max(0.8);
        let hard = s.hard.clamp(0.0, 1.0);
        let feather = ((1.0 - hard) * r).max(1.0);
        let strength = s.strength.clamp(0.0, 1.0);
        let pts: Vec<(f32, f32)> = s.pts.iter().map(|&(x, y)| (x * k, y * k)).collect();
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in &pts {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        let x0 = ((minx - r - 2.0).floor() as i32).max(0);
        let y0 = ((miny - r - 2.0).floor() as i32).max(0);
        let x1 = ((maxx + r + 2.0).ceil() as i32).min(aw - 1);
        let y1 = ((maxy + r + 2.0).ceil() as i32).min(ah - 1);
        if x1 < x0 || y1 < y0 {
            continue;
        }
        let (bw, bh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
        let mut cov = vec![0f32; bw * bh];
        let mut stamp = |cx: f32, cy: f32| {
            let sx0 = ((cx - r - 1.0).floor() as i32).max(x0);
            let sx1 = ((cx + r + 1.0).ceil() as i32).min(x1);
            let sy0 = ((cy - r - 1.0).floor() as i32).max(y0);
            let sy1 = ((cy + r + 1.0).ceil() as i32).min(y1);
            for y in sy0..=sy1 {
                for x in sx0..=sx1 {
                    let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                    let t = ((r + 0.5 - d) / feather).clamp(0.0, 1.0);
                    if t <= 0.0 {
                        continue;
                    }
                    let c = t * t * (3.0 - 2.0 * t); // smooth ramp
                    let p = &mut cov[(y - y0) as usize * bw + (x - x0) as usize];
                    if c > *p {
                        *p = c;
                    }
                }
            }
        };
        let mut prev: Option<(f32, f32)> = None;
        for &(x, y) in &pts {
            match prev {
                None => stamp(x, y),
                Some((px, py)) => {
                    let dist = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
                    let n = (dist / (r * 0.25).max(0.6)).ceil().max(1.0) as i32;
                    for i in 1..=n {
                        let t = i as f32 / n as f32;
                        stamp(px + (x - px) * t, py + (y - py) * t);
                    }
                }
            }
            prev = Some((x, y));
        }
        let raw = alpha.as_mut();
        for y in 0..bh {
            for x in 0..bw {
                let c = cov[y * bw + x] * strength;
                if c <= 0.0 {
                    continue;
                }
                let px = &mut raw[(y as i32 + y0) as usize * aw as usize + (x as i32 + x0) as usize];
                let v = *px as f32;
                *px = if s.erase { v * (1.0 - c) } else { v + (255.0 - v) * c }.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Coverage (0..1) of a polygon inside the box `[x0, y0, x0+bw, y0+bh)`, anti-aliased (4 sub-rows, exact spans).
fn polygon_coverage(pts: &[(f32, f32)], x0: i32, y0: i32, bw: usize, bh: usize) -> Vec<f32> {
    let mut cov = vec![0f32; bw * bh];
    let n = pts.len();
    for row in 0..bh {
        for sub in 0..4 {
            let yy = (y0 as usize + row) as f32 + (sub as f32 + 0.5) / 4.0;
            let mut xs: Vec<f32> = Vec::new();
            for i in 0..n {
                let (ax, ay) = pts[i];
                let (bx, by) = pts[(i + 1) % n];
                if (ay <= yy && by > yy) || (by <= yy && ay > yy) {
                    xs.push(ax + (yy - ay) / (by - ay) * (bx - ax));
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            for pair in xs.chunks(2) {
                if pair.len() < 2 {
                    break;
                }
                let (sx, ex) = (pair[0] - x0 as f32, pair[1] - x0 as f32);
                let first = sx.floor().max(0.0) as usize;
                let last = (ex.ceil() as i64).clamp(0, bw as i64) as usize;
                for x in first..last {
                    let ov = (ex.min(x as f32 + 1.0) - sx.max(x as f32)).max(0.0);
                    cov[row * bw + x] += ov * 0.25;
                }
            }
        }
    }
    cov
}

/// Apply Pen / Lasso shapes (erase inside, restore inside, or keep only inside) with a soft edge.
pub fn apply_cuts(alpha: &mut GrayImage, src_w: u32, cuts: &[PolyCut]) {
    let k = alpha.width() as f32 / src_w as f32;
    let (aw, ah) = (alpha.width() as i32, alpha.height() as i32);
    for c in cuts {
        if c.pts.len() < 3 {
            continue;
        }
        let pts: Vec<(f32, f32)> = c.pts.iter().map(|&(x, y)| (x * k, y * k)).collect();
        let sigma = c.feather * k * 0.5;
        let pad = if sigma > 0.3 { (sigma * 3.0).ceil() as i32 + 2 } else { 2 };
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in &pts {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        let x0 = (minx.floor() as i32 - pad).max(0);
        let y0 = (miny.floor() as i32 - pad).max(0);
        let x1 = (maxx.ceil() as i32 + pad).min(aw - 1);
        let y1 = (maxy.ceil() as i32 + pad).min(ah - 1);
        if x1 < x0 || y1 < y0 {
            if c.mode == CutMode::KeepInside {
                alpha.as_mut().fill(0);
            }
            continue;
        }
        let (bw, bh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
        let cov = polygon_coverage(&pts, x0, y0, bw, bh);
        let mut m = GrayImage::from_fn(bw as u32, bh as u32, |x, y| image::Luma([(cov[y as usize * bw + x as usize].clamp(0.0, 1.0) * 255.0).round() as u8]));
        if sigma > 0.3 {
            m = image::imageops::blur(&m, sigma);
        }
        let raw = alpha.as_mut();
        if c.mode == CutMode::KeepInside {
            // everything outside the box is removed, inside it follows the (soft) shape
            for y in 0..ah {
                for x in 0..aw {
                    let inside = x >= x0 && x <= x1 && y >= y0 && y <= y1;
                    let px = &mut raw[(y * aw + x) as usize];
                    if !inside {
                        *px = 0;
                    } else {
                        let mv = m.get_pixel((x - x0) as u32, (y - y0) as u32).0[0] as f32 / 255.0;
                        *px = (*px as f32 * mv).round() as u8;
                    }
                }
            }
            continue;
        }
        for y in 0..bh {
            for x in 0..bw {
                let mv = m.get_pixel(x as u32, y as u32).0[0] as f32 / 255.0;
                if mv <= 0.0 {
                    continue;
                }
                let px = &mut raw[(y as i32 + y0) as usize * aw as usize + (x as i32 + x0) as usize];
                let v = *px as f32;
                *px = if c.mode == CutMode::Erase { v * (1.0 - mv) } else { v + (255.0 - v) * mv }.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn cover_fit(im: &RgbImage, pw: u32, ph: u32) -> RgbImage {
    if im.dimensions() == (pw, ph) {
        return im.clone();
    }
    let s = (pw as f32 / im.width() as f32).max(ph as f32 / im.height() as f32);
    let (nw, nh) = (((im.width() as f32 * s).round() as u32).max(pw), ((im.height() as f32 * s).round() as u32).max(ph));
    let big = image::imageops::resize(im, nw, nh, image::imageops::FilterType::Lanczos3);
    image::imageops::crop_imm(&big, (nw - pw) / 2, (nh - ph) / 2, pw, ph).to_image()
}

/// Two photos in one frame. `a` / `b` are already rendered for their own cell. B = A when `b` is None.
pub fn compose_duet(a: &RgbImage, b: Option<&RgbImage>, e: &Edit) -> RgbImage {
    let (tw, th) = e.duet_px();
    let cells = e.duet_cells();
    let mut out = RgbImage::from_pixel(tw, th, image::Rgb([255, 255, 255]));
    let (ca, cb) = (cells[0], cells[1]);
    let fa = cover_fit(a, ca.2, ca.3);
    let fb = cover_fit(b.unwrap_or(a), cb.2, cb.3);
    image::imageops::replace(&mut out, &fa, ca.0 as i64, ca.1 as i64);
    // the photo being edited is drawn on top
    image::imageops::replace(&mut out, &fb, cb.0 as i64, cb.1 as i64);
    if e.duet_free && e.duet_active == 0 {
        image::imageops::replace(&mut out, &fa, ca.0 as i64, ca.1 as i64);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_brush_and_pen_shapes() {
        let mut a = GrayImage::from_pixel(100, 100, image::Luma([255]));
        // soft + half strength erase: centre is only partly removed, never fully
        let soft = BrushStroke { erase: true, r: 12.0, pts: vec![(50.0, 50.0)], hard: 0.0, strength: 0.5 };
        apply_strokes(&mut a, 100, &[soft]);
        let c = a.get_pixel(50, 50).0[0];
        assert!(c > 100 && c < 150, "centre {c}");
        // pen shape: erase the square 20..60, sharp edge
        let mut b = GrayImage::from_pixel(100, 100, image::Luma([255]));
        let sq = PolyCut { mode: CutMode::Erase, pts: vec![(20.0, 20.0), (60.0, 20.0), (60.0, 60.0), (20.0, 60.0)], feather: 0.0 };
        apply_cuts(&mut b, 100, &[sq.clone()]);
        assert_eq!(b.get_pixel(40, 40).0[0], 0);
        assert_eq!(b.get_pixel(80, 80).0[0], 255);
        // keep only inside
        let mut d = GrayImage::from_pixel(100, 100, image::Luma([255]));
        apply_cuts(&mut d, 100, &[PolyCut { mode: CutMode::KeepInside, ..sq.clone() }]);
        assert_eq!(d.get_pixel(40, 40).0[0], 255);
        assert_eq!(d.get_pixel(80, 80).0[0], 0);
        // feather: edge pixel is partly removed
        let mut f = GrayImage::from_pixel(100, 100, image::Luma([255]));
        apply_cuts(&mut f, 100, &[PolyCut { feather: 10.0, ..sq }]);
        let e = f.get_pixel(60, 40).0[0];
        assert!(e > 20 && e < 235, "edge {e}");
    }

    #[test]
    fn duet_bank_size_is_570_x_450() {
        let mut e = Edit::default();
        e.duet = true;
        assert_eq!(e.duet_px(), (570, 450));
        assert_eq!(e.photo_px(), (285, 450));
        e.duet_stack = true;
        assert_eq!(e.photo_px(), (570, 225));
        e.duet_stack = false;
        e.duet_between_mm = 2.0;
        let (pw, _) = e.photo_px();
        assert_eq!(2 * pw + e.duet_between_px(), 570 - (570 - e.duet_between_px()) % 2);
    }
    #[test]
    fn rotate_and_flip_change_the_photo() {
        let mut src = RgbImage::from_pixel(200, 200, image::Rgb([0, 0, 0]));
        for y in 0..200 {
            for x in 0..100 {
                src.put_pixel(x, y, image::Rgb([250, 250, 250])); // left half white
            }
        }
        let mut e = Edit::default();
        e.auto_enhance = false;
        e.sharpen = 0.0;
        e.reset_crop(200, 200);
        let base = render_photo(&src, None, &e, None);
        let (w, h) = base.dimensions();
        assert!(base.get_pixel(5, h / 2)[0] > 200 && base.get_pixel(w - 5, h / 2)[0] < 50);
        e.flip_h = true;
        let f = render_photo(&src, None, &e, None);
        assert!(f.get_pixel(5, h / 2)[0] < 50 && f.get_pixel(w - 5, h / 2)[0] > 200);
        e.flip_h = false;
        e.rotate_quarter(1); // right: the white left half goes to the top
        let r = render_photo(&src, None, &e, None);
        assert!(r.get_pixel(w / 2, 5)[0] > 200 && r.get_pixel(w / 2, h - 5)[0] < 50, "{} {}", r.get_pixel(w / 2, 5)[0], r.get_pixel(w / 2, h - 5)[0]);
        e.rotate_quarter(-1);
        assert!((e.angle).abs() < 0.01);
    }
    #[test]
    fn duet_composes_two_different_photos() {
        let mut e = Edit::default();
        e.duet = true;
        let (pw, ph) = e.photo_px();
        let a = RgbImage::from_pixel(pw, ph, image::Rgb([255, 0, 0]));
        let b = RgbImage::from_pixel(pw, ph, image::Rgb([0, 0, 255]));
        let d = compose_duet(&a, Some(&b), &e);
        assert_eq!(d.dimensions(), (570, 450));
        assert_eq!(d.get_pixel(10, 10).0, [255, 0, 0]);
        assert_eq!(d.get_pixel(560, 10).0, [0, 0, 255]);
    }
    #[test]
    fn brush_restores_and_erases() {
        let mut a = GrayImage::from_pixel(100, 100, image::Luma([0]));
        let restore = BrushStroke { erase: false, r: 10.0, pts: vec![(50.0, 50.0), (80.0, 50.0)], hard: 1.0, strength: 1.0 };
        apply_strokes(&mut a, 100, &[restore]);
        assert_eq!(a.get_pixel(50, 50)[0], 255);
        assert_eq!(a.get_pixel(70, 50)[0], 255); // along the segment
        assert_eq!(a.get_pixel(10, 10)[0], 0);
        let erase = BrushStroke { erase: true, r: 5.0, pts: vec![(50.0, 50.0)], hard: 1.0, strength: 1.0 };
        apply_strokes(&mut a, 100, &[erase]);
        assert_eq!(a.get_pixel(50, 50)[0], 0);
        assert_eq!(a.get_pixel(70, 50)[0], 255);
    }
    #[test]
    fn erase_brush_works_without_ai_matte() {
        // no matte at all: manual erase must cut the background to the bg colour
        let src = RgbImage::from_pixel(200, 200, image::Rgb([10, 10, 10]));
        let mut e = Edit::default();
        e.reset_crop(200, 200);
        e.auto_enhance = false;
        e.sharpen = 0.0;
        e.size_idx = CUSTOM_IDX;
        e.custom_w_mm = 20.0;
        e.custom_h_mm = 20.0;
        e.bg = [255, 255, 255];
        e.strokes.push(BrushStroke { erase: true, r: 60.0, pts: vec![(100.0, 100.0)], hard: 1.0, strength: 1.0 });
        let out = render_photo(&src, None, &e, None);
        let (w, h) = out.dimensions();
        assert!(out.get_pixel(w / 2, h / 2)[0] > 240, "centre should be erased to white");
        assert!(out.get_pixel(2, 2)[0] < 40, "corner should stay original");
    }

    #[test]
    fn duet_free_layout_cells_and_compose() {
        let mut e = Edit::default();
        e.duet = true;
        let auto = e.duet_cells();
        e.duet_make_free();
        assert_eq!(e.duet_cells(), auto);
        e.duet_rect[1] = [0.5, 0.25, 0.25, 0.5];
        e.duet_active = 1;
        assert_eq!(e.photo_px(), (143, 225));
        e.duet_active = 0;
        assert_eq!(e.photo_px(), (285, 450));
        let a = RgbImage::from_pixel(285, 450, image::Rgb([200, 0, 0]));
        let b = RgbImage::from_pixel(143, 225, image::Rgb([0, 0, 200]));
        let out = compose_duet(&a, Some(&b), &e);
        assert_eq!(out.get_pixel(10, 10).0, [200, 0, 0]);
        assert_eq!(out.get_pixel(300, 200).0, [0, 0, 200]);
        assert_eq!(out.get_pixel(560, 10).0, [255, 255, 255]);
    }
    #[test]
    fn swap_photo_keeps_frame_settings() {
        let mut a = Edit::default();
        let mut b = Edit::default();
        a.brightness = 0.3;
        a.dpi = 600;
        b.angle = 20.0;
        a.swap_photo(&mut b);
        assert_eq!(a.angle, 20.0);
        assert_eq!(b.brightness, 0.3);
        assert_eq!(a.dpi, 600);
        assert_eq!(b.dpi, 300);
    }
}
