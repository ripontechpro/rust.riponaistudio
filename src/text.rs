//! Tiny text renderer for captions (name / date under the photo, studio name on sheets).
use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use image::RgbImage;
use std::sync::OnceLock;

static FONT: OnceLock<Option<FontVec>> = OnceLock::new();

fn candidates() -> Vec<&'static str> {
    vec![
        r"C:\Windows\Fonts\segoeuib.ttf",
        r"C:\Windows\Fonts\arialbd.ttf",
        r"C:\Windows\Fonts\calibrib.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\arial.ttf",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/Library/Fonts/Arial Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    ]
}

static EMBEDDED: &[u8] = include_bytes!("../assets/fonts/Poppins-SemiBold.ttf");

pub fn font() -> Option<&'static FontVec> {
    FONT.get_or_init(|| {
        if let Ok(f) = FontVec::try_from_vec(EMBEDDED.to_vec()) {
            return Some(f);
        }
        for p in candidates() {
            if let Ok(b) = std::fs::read(p) {
                if let Ok(f) = FontVec::try_from_vec(b) {
                    return Some(f);
                }
            }
        }
        None
    })
    .as_ref()
}

pub fn text_width(text: &str, px: f32) -> f32 {
    let Some(f) = font() else { return 0.0 };
    let sf = f.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Draw `text` horizontally centred at `cx`, with its vertical middle at `cy`. Shrinks to fit `max_w`.
pub fn draw_centered(img: &mut RgbImage, text: &str, cx: f32, cy: f32, mut px: f32, max_w: f32, color: [u8; 3]) -> bool {
    let Some(f) = font() else { return false };
    if text.trim().is_empty() {
        return true;
    }
    let tw = text_width(text, px);
    if tw > max_w && tw > 0.0 {
        px *= max_w / tw;
    }
    let sf = f.as_scaled(PxScale::from(px));
    let width = text_width(text, px);
    let mut x = cx - width / 2.0;
    let baseline = cy + (sf.ascent() + sf.descent()) / 2.0;
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            x += sf.kern(p, id);
        }
        let glyph = id.with_scale_and_position(PxScale::from(px), point(x, baseline));
        x += sf.h_advance(id);
        prev = Some(id);
        if let Some(og) = f.outline_glyph(glyph) {
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                let (ix, iy) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if ix >= 0 && iy >= 0 && (ix as u32) < img.width() && (iy as u32) < img.height() {
                    let p = img.get_pixel_mut(ix as u32, iy as u32);
                    for k in 0..3 {
                        p[k] = (p[k] as f32 * (1.0 - cov) + color[k] as f32 * cov) as u8;
                    }
                }
            });
        }
    }
    true
}
