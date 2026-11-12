//! Print sheet layout (many copies on A4 / 4R / ...) and export (JPG, PNG, PDF, "under N KB").
use crate::text;
use image::{codecs::jpeg::JpegEncoder, imageops, ImageEncoder, Rgb, RgbImage};
use serde::{Deserialize, Serialize};

pub struct Paper {
    pub name: &'static str,
    pub w_mm: f32,
    pub h_mm: f32,
}
pub const PAPERS: &[Paper] = &[
    Paper { name: "A4  (210 x 297 mm)", w_mm: 210.0, h_mm: 297.0 },
    Paper { name: "4R / 4x6 inch  (102 x 152 mm)", w_mm: 101.6, h_mm: 152.4 },
    Paper { name: "5R / 5x7 inch  (127 x 178 mm)", w_mm: 127.0, h_mm: 177.8 },
    Paper { name: "A3  (297 x 420 mm)", w_mm: 297.0, h_mm: 420.0 },
    Paper { name: "A5  (148 x 210 mm)", w_mm: 148.0, h_mm: 210.0 },
    Paper { name: "A6  (105 x 148 mm)", w_mm: 105.0, h_mm: 148.0 },
    Paper { name: "Letter  (216 x 279 mm)", w_mm: 215.9, h_mm: 279.4 },
    Paper { name: "Legal  (216 x 356 mm)", w_mm: 215.9, h_mm: 355.6 },
    Paper { name: "Custom paper (mm)", w_mm: 210.0, h_mm: 297.0 },
];
pub const CUSTOM_PAPER: usize = 8;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug)]
pub enum Border {
    None,
    Thin,
    CutMarks,
}
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug)]
pub enum Align {
    TopLeft,
    Center,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SheetOpts {
    pub paper_idx: usize,
    pub custom_w_mm: f32,
    pub custom_h_mm: f32,
    pub copies: u32,
    pub fill_page: bool,
    pub gap_mm: f32,
    pub margin_mm: f32,
    pub border: Border,
    pub align: Align,
    pub auto_rotate: bool,
    pub studio_footer: bool,
}
impl Default for SheetOpts {
    fn default() -> Self {
        SheetOpts {
            paper_idx: 0,
            custom_w_mm: 210.0,
            custom_h_mm: 297.0,
            copies: 8,
            fill_page: false,
            gap_mm: 3.0,
            margin_mm: 6.0,
            border: Border::Thin,
            align: Align::TopLeft,
            auto_rotate: true,
            studio_footer: false,
        }
    }
}
impl SheetOpts {
    pub fn paper_mm(&self) -> (f32, f32) {
        if self.paper_idx == CUSTOM_PAPER {
            (self.custom_w_mm.clamp(40.0, 1000.0), self.custom_h_mm.clamp(40.0, 1000.0))
        } else {
            let p = &PAPERS[self.paper_idx.min(PAPERS.len() - 1)];
            (p.w_mm, p.h_mm)
        }
    }
}

pub(crate) fn mm_px(mm: f32, dpi: u32) -> u32 {
    (mm / 25.4 * dpi as f32).round().max(1.0) as u32
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub page_w: u32,
    pub page_h: u32,
    pub cols: u32,
    pub rows: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    pub rotated: bool,
    pub ox: u32,
    pub oy: u32,
    pub gap: u32,
}
impl Layout {
    pub fn per_page(&self) -> u32 {
        self.cols * self.rows
    }
}

pub fn best_layout(photo_w: u32, photo_h: u32, o: &SheetOpts, dpi: u32) -> Layout {
    let (pmw, pmh) = o.paper_mm();
    let (pw, ph) = (mm_px(pmw, dpi), mm_px(pmh, dpi));
    let gap = mm_px(o.gap_mm.max(0.0), dpi).min(mm_px(30.0, dpi));
    let margin = if o.margin_mm <= 0.0 { 0 } else { mm_px(o.margin_mm, dpi) };
    let mut best: Option<Layout> = None;
    for landscape in [false, true] {
        for rotated in if o.auto_rotate { vec![false, true] } else { vec![false] } {
            let (pgw, pgh) = if landscape { (ph, pw) } else { (pw, ph) };
            let (cw, ch) = if rotated { (photo_h, photo_w) } else { (photo_w, photo_h) };
            let availw = pgw.saturating_sub(2 * margin);
            let availh = pgh.saturating_sub(2 * margin);
            if availw < cw || availh < ch {
                continue;
            }
            let cols = (availw + gap) / (cw + gap);
            let rows = (availh + gap) / (ch + gap);
            let count = cols * rows;
            let better = match &best {
                None => true,
                Some(b) => count > b.per_page(),
            };
            if better {
                let usedw = cols * cw + (cols - 1) * gap;
                let usedh = rows * ch + (rows - 1) * gap;
                let (ox, oy) = match o.align {
                    Align::TopLeft => (margin, margin),
                    Align::Center => (margin + (availw - usedw) / 2, margin + (availh - usedh) / 2),
                };
                best = Some(Layout { page_w: pgw, page_h: pgh, cols, rows, cell_w: cw, cell_h: ch, rotated, ox, oy, gap });
            }
        }
    }
    best.unwrap_or(Layout { page_w: pw, page_h: ph, cols: 0, rows: 0, cell_w: photo_w, cell_h: photo_h, rotated: false, ox: 0, oy: 0, gap })
}

pub(crate) fn draw_rect(img: &mut RgbImage, x: i64, y: i64, w: u32, h: u32, t: u32, c: Rgb<u8>) {
    let put = |img: &mut RgbImage, px: i64, py: i64| {
        if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
            img.put_pixel(px as u32, py as u32, c);
        }
    };
    for k in 0..t as i64 {
        for i in 0..w as i64 {
            put(img, x + i, y + k);
            put(img, x + i, y + h as i64 - 1 - k);
        }
        for i in 0..h as i64 {
            put(img, x + k, y + i);
            put(img, x + w as i64 - 1 - k, y + i);
        }
    }
}

pub(crate) fn cut_marks(img: &mut RgbImage, x: i64, y: i64, w: u32, h: u32, len: i64, off: i64, t: u32) {
    let c = Rgb([60, 60, 60]);
    let (x2, y2) = (x + w as i64, y + h as i64);
    let mut line = |ax: i64, ay: i64, bx: i64, by: i64| {
        for k in 0..t as i64 {
            let (mut px, mut py) = (ax, ay);
            while px != bx || py != by {
                let (qx, qy) = if ax == bx { (px + k, py) } else { (px, py + k) };
                if qx >= 0 && qy >= 0 && (qx as u32) < img.width() && (qy as u32) < img.height() {
                    img.put_pixel(qx as u32, qy as u32, c);
                }
                px += (bx - ax).signum();
                py += (by - ay).signum();
            }
        }
    };
    for (cx, cy, dx, dy) in [(x, y, -1, -1), (x2, y, 1, -1), (x, y2, -1, 1), (x2, y2, 1, 1)] {
        line(cx + dx * off, cy, cx + dx * (off + len), cy);
        line(cx, cy + dy * off, cx, cy + dy * (off + len));
    }
}

/// Number of photos and pages for the info line in the UI.
pub fn plan(photo_w: u32, photo_h: u32, o: &SheetOpts, dpi: u32) -> (Layout, u32, u32) {
    let l = best_layout(photo_w, photo_h, o, dpi);
    let per = l.per_page();
    if per == 0 {
        return (l, 0, 0);
    }
    let copies = if o.fill_page { per } else { o.copies.max(1) };
    let pages = (copies + per - 1) / per;
    (l, copies, pages)
}

pub fn make_sheets(photo: &RgbImage, o: &SheetOpts, dpi: u32) -> Result<Vec<RgbImage>, String> {
    let (l, copies, pages) = plan(photo.width(), photo.height(), o, dpi);
    if l.per_page() == 0 {
        return Err("The photo is bigger than the paper. Choose a larger paper or smaller margin.".into());
    }
    let tile = if l.rotated { imageops::rotate90(photo) } else { photo.clone() };
    let mut out = Vec::new();
    let mut left = copies;
    let line_t = (dpi as f32 / 150.0).round().max(1.0) as u32;
    for _ in 0..pages {
        let mut page = RgbImage::from_pixel(l.page_w, l.page_h, Rgb([255, 255, 255]));
        'fill: for r in 0..l.rows {
            for c in 0..l.cols {
                if left == 0 {
                    break 'fill;
                }
                let x = (l.ox + c * (l.cell_w + l.gap)) as i64;
                let y = (l.oy + r * (l.cell_h + l.gap)) as i64;
                imageops::replace(&mut page, &tile, x, y);
                match o.border {
                    Border::None => {}
                    Border::Thin => draw_rect(&mut page, x, y, l.cell_w, l.cell_h, line_t, Rgb([150, 150, 150])),
                    Border::CutMarks => {
                        let off = (l.gap / 6).max(2) as i64;
                        let len = ((l.gap as i64 * 2) / 3 - off).clamp(4, mm_px(4.0, dpi) as i64);
                        cut_marks(&mut page, x, y, l.cell_w, l.cell_h, len, off, line_t)
                    }
                }
                left -= 1;
            }
        }
        if o.studio_footer {
            let y = l.page_h as f32 - mm_px(3.0, dpi) as f32;
            text::draw_centered(&mut page, "Ripon AI Photo Studio", l.page_w as f32 / 2.0, y, mm_px(2.2, dpi) as f32, l.page_w as f32 * 0.8, [150, 150, 150]);
        }
        out.push(page);
    }
    Ok(out)
}

// ------------------------------------------------------------------ encoders
pub fn encode_jpeg(img: &RgbImage, quality: u8, dpi: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
    enc.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(dpi.clamp(1, 65000) as u16));
    let _ = enc.write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8);
    buf
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// PNG with a pHYs chunk so the DPI is stored.
pub fn encode_png(img: &RgbImage, dpi: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = image::codecs::png::PngEncoder::new(&mut buf).write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8);
    // insert pHYs after IHDR (8 sig + 25 IHDR)
    if buf.len() > 33 {
        let ppm = (dpi as f64 / 0.0254).round() as u32;
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"pHYs");
        chunk.extend_from_slice(&ppm.to_be_bytes());
        chunk.extend_from_slice(&ppm.to_be_bytes());
        chunk.push(1);
        let mut full = Vec::new();
        full.extend_from_slice(&9u32.to_be_bytes());
        full.extend_from_slice(&chunk);
        full.extend_from_slice(&crc32(&chunk).to_be_bytes());
        let mut out = buf[..33].to_vec();
        out.extend_from_slice(&full);
        out.extend_from_slice(&buf[33..]);
        return out;
    }
    buf
}

/// Multi-page PDF, one JPEG per page, exact physical page size.
pub fn encode_pdf(pages: &[RgbImage], dpi: u32, quality: u8) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offs: Vec<usize> = Vec::new();
    let n = pages.len();
    let obj = |out: &mut Vec<u8>, offs: &mut Vec<usize>, body: &[u8]| {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offs.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, &mut offs, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 3 + 3 * i)).collect();
    obj(&mut out, &mut offs, format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").as_bytes());
    for (i, pg) in pages.iter().enumerate() {
        let (wpt, hpt) = (pg.width() as f64 / dpi as f64 * 72.0, pg.height() as f64 / dpi as f64 * 72.0);
        let (cont_id, img_id) = (4 + 3 * i, 5 + 3 * i);
        obj(&mut out, &mut offs, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {wpt:.2} {hpt:.2}] /Contents {cont_id} 0 R /Resources << /XObject << /Im0 {img_id} 0 R >> >> >>").as_bytes());
        let content = format!("q {wpt:.2} 0 0 {hpt:.2} 0 0 cm /Im0 Do Q");
        obj(&mut out, &mut offs, format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), content).as_bytes());
        let jpg = encode_jpeg(pg, quality, dpi);
        let mut body = format!("<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", pg.width(), pg.height(), jpg.len()).into_bytes();
        body.extend_from_slice(&jpg);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &mut offs, &body);
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offs.len() + 1).as_bytes());
    for o in &offs {
        out.extend_from_slice(format!("{:010} 00000 n \n", o).as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", offs.len() + 1, xref).as_bytes());
    out
}

/// "Save for online": resize to w x h (or keep size) and find the best JPEG quality under `max_kb`.
/// Returns (bytes, quality, fits).
pub fn jpeg_under_kb(img: &RgbImage, size: Option<(u32, u32)>, max_kb: u32, dpi: u32) -> (Vec<u8>, u8, bool) {
    let im = match size {
        Some((w, h)) if (w, h) != img.dimensions() => imageops::resize(img, w.max(8), h.max(8), imageops::FilterType::Lanczos3),
        _ => img.clone(),
    };
    let limit = max_kb as usize * 1024;
    let (mut lo, mut hi) = (10u8, 98u8);
    let mut best = encode_jpeg(&im, lo, dpi);
    if best.len() > limit {
        return (best, lo, false);
    }
    let mut bq = lo;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let b = encode_jpeg(&im, mid, dpi);
        if b.len() <= limit {
            best = b;
            bq = mid;
            lo = mid + 1;
        } else {
            if mid == 0 {
                break;
            }
            hi = mid - 1;
        }
    }
    (best, bq, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_fits_expected_count() {
        // 35x45 mm @300dpi = 413x531 ; A4, 6 mm margin, 3 mm gap -> 5 x 6
        let l = best_layout(413, 531, &SheetOpts::default(), 300);
        assert_eq!((l.cols, l.rows), (5, 6));
        assert!(!l.rotated);
    }
    #[test]
    fn photos_never_leave_the_page() {
        for idx in 0..PAPERS.len() {
            let o = SheetOpts { paper_idx: idx, ..Default::default() };
            let l = best_layout(413, 531, &o, 300);
            if l.per_page() > 0 {
                let right = l.ox + l.cols * l.cell_w + (l.cols - 1) * l.gap;
                let bottom = l.oy + l.rows * l.cell_h + (l.rows - 1) * l.gap;
                assert!(right <= l.page_w && bottom <= l.page_h, "paper {idx}");
            }
        }
    }
    #[test]
    fn copies_spill_to_more_pages() {
        let o = SheetOpts { copies: 70, ..Default::default() };
        let (_, copies, pages) = plan(413, 531, &o, 300);
        assert_eq!((copies, pages), (70, 3));
    }
    #[test]
    fn pdf_and_png_are_well_formed() {
        let img = RgbImage::from_pixel(120, 160, Rgb([200, 100, 50]));
        let pdf = encode_pdf(&[img.clone(), img.clone()], 300, 90);
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.windows(5).any(|w| w == b"%%EOF"));
        assert!(String::from_utf8_lossy(&pdf).contains("/Count 2"));
        let png = encode_png(&img, 300);
        let back = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(back.dimensions(), (120, 160));
        assert!(png.windows(4).any(|w| w == b"pHYs"));
    }
    #[test]
    fn kb_limit_is_respected() {
        let mut img = RgbImage::new(600, 800);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([(x * 7 % 256) as u8, (y * 5 % 256) as u8, ((x * y) % 256) as u8]);
        }
        let (bytes, _q, fits) = jpeg_under_kb(&img, Some((354, 472)), 50, 72);
        assert!(fits && bytes.len() <= 50 * 1024, "len {}", bytes.len());
    }
}
