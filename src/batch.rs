//! Multi-customer print sheets: many different photos, each with its own copy count, packed onto
//! as few sheets as possible (shelf packing), with real-size output and no stretching.
use crate::sheet::{self, Align, Border, SheetOpts};
use crate::text;
use image::{imageops, Rgb, RgbImage};

/// One customer photo: pixel size at the output DPI and how many copies are wanted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub w: u32,
    pub h: u32,
    pub copies: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub page: usize,
    pub item: usize,
    pub x: i64,
    pub y: i64,
    pub w: u32,
    pub h: u32,
    pub rotated: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub page_w: u32,
    pub page_h: u32,
    pub pages: usize,
    pub placed: Vec<Placed>,
    pub total: u32,
    /// items (index) that are larger than the paper and were skipped
    pub too_big: Vec<usize>,
    /// share of the paper area that carries photos, 0..1
    pub used: f32,
}

fn pack(tiles: &[(usize, u32, u32)], pw: u32, ph: u32, margin: u32, gap: u32, rotate: bool) -> (Vec<Placed>, usize, Vec<usize>) {
    let mut out = Vec::new();
    let mut big = Vec::new();
    let (mut x, mut y, mut rowh, mut page) = (margin, margin, 0u32, 0usize);
    let (iw, ih) = (pw.saturating_sub(2 * margin), ph.saturating_sub(2 * margin));
    for &(item, w0, h0) in tiles {
        let (w, h) = if rotate { (h0, w0) } else { (w0, h0) };
        if w > iw || h > ih {
            if !big.contains(&item) {
                big.push(item);
            }
            continue;
        }
        if x > margin && x + w > pw - margin {
            x = margin;
            y += rowh + gap;
            rowh = 0;
        }
        if y + h > ph - margin {
            page += 1;
            x = margin;
            y = margin;
            rowh = 0;
        }
        out.push(Placed { page, item, x: x as i64, y: y as i64, w, h, rotated: rotate });
        x += w + gap;
        rowh = rowh.max(h);
    }
    let pages = if out.is_empty() { 0 } else { out.last().unwrap().page + 1 };
    (out, pages, big)
}

/// Work out where every photo goes. `landscape` turns the paper; `optimise` lets the planner sort by size
/// and try rotated photos to save paper (customers with the same size stay together).
pub fn plan(items: &[Item], o: &SheetOpts, landscape: bool, dpi: u32, optimise: bool) -> Plan {
    let (mw, mh) = o.paper_mm();
    let (mw, mh) = if landscape { (mh.max(mw), mh.min(mw)) } else { (mw.min(mh), mh.max(mw)) };
    let (pw, ph) = (sheet::mm_px(mw, dpi), sheet::mm_px(mh, dpi));
    let (margin, gap) = (sheet::mm_px(o.margin_mm, dpi).min(pw / 3), if o.gap_mm <= 0.0 { 0 } else { sheet::mm_px(o.gap_mm, dpi) });
    let mut tiles: Vec<(usize, u32, u32)> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        for _ in 0..it.copies.min(999) {
            tiles.push((i, it.w, it.h));
        }
    }
    let total = tiles.len() as u32;
    if optimise {
        // tallest first, same sizes keep customer order
        tiles.sort_by_key(|t| (std::cmp::Reverse(t.2), std::cmp::Reverse(t.1), t.0));
    }
    let (a, pa, ba) = pack(&tiles, pw, ph, margin, gap, false);
    let (mut best, mut pages, mut big) = (a, pa, ba);
    if o.auto_rotate && optimise {
        let (b, pb, bb) = pack(&tiles, pw, ph, margin, gap, true);
        // rotate only when it really saves a sheet (or makes a photo fit) - upright photos are easier to cut
        let better = bb.len() < big.len() || (bb.len() == big.len() && pb < pages);
        if better {
            best = b;
            pages = pb;
            big = bb;
        }
    }
    // centre the block when asked
    if o.align == Align::Center {
        for pg in 0..pages {
            let mut ys: Vec<i64> = best.iter().filter(|p| p.page == pg).map(|p| p.y).collect();
            ys.sort();
            ys.dedup();
            let top = best.iter().filter(|p| p.page == pg).map(|p| p.y).min().unwrap_or(0);
            let bottom = best.iter().filter(|p| p.page == pg).map(|p| p.y + p.h as i64).max().unwrap_or(0);
            let dy = ((ph as i64 - (bottom - top)) / 2) - top;
            for y in &ys {
                let row: Vec<usize> = best.iter().enumerate().filter(|(_, p)| p.page == pg && p.y == *y).map(|(i, _)| i).collect();
                let left = row.iter().map(|&i| best[i].x).min().unwrap_or(0);
                let right = row.iter().map(|&i| best[i].x + best[i].w as i64).max().unwrap_or(0);
                let dx = (pw as i64 - (right - left)) / 2 - left;
                for &i in &row {
                    best[i].x += dx;
                    best[i].y += dy;
                }
            }
        }
    }
    let used_area: f64 = best.iter().map(|p| p.w as f64 * p.h as f64).sum();
    let used = if pages == 0 { 0.0 } else { (used_area / (pages as f64 * pw as f64 * ph as f64)) as f32 };
    Plan { page_w: pw, page_h: ph, pages, placed: best, total, too_big: big, used }
}

/// Draw every page. `imgs[i]` is the finished photo of item `i`.
pub fn render(plan: &Plan, imgs: &[&RgbImage], o: &SheetOpts, dpi: u32) -> Vec<RgbImage> {
    let line_t = (dpi as f32 / 150.0).round().max(1.0) as u32;
    let gap = sheet::mm_px(o.gap_mm.max(0.0), dpi);
    let mut cache: Vec<Option<RgbImage>> = vec![None; imgs.len()];
    let mut pages = Vec::new();
    for pg in 0..plan.pages {
        let mut page = RgbImage::from_pixel(plan.page_w, plan.page_h, Rgb([255, 255, 255]));
        for p in plan.placed.iter().filter(|p| p.page == pg) {
            let Some(img) = imgs.get(p.item) else { continue };
            let tile: &RgbImage = if p.rotated {
                cache[p.item].get_or_insert_with(|| imageops::rotate90(*img))
            } else {
                img
            };
            imageops::replace(&mut page, tile, p.x, p.y);
            match o.border {
                Border::None => {}
                Border::Thin => sheet::draw_rect(&mut page, p.x, p.y, p.w, p.h, line_t, Rgb([150, 150, 150])),
                Border::CutMarks => {
                    let off = (gap / 6).max(2) as i64;
                    let len = ((gap as i64 * 2) / 3 - off).clamp(4, sheet::mm_px(4.0, dpi) as i64);
                    sheet::cut_marks(&mut page, p.x, p.y, p.w, p.h, len, off, line_t)
                }
            }
        }
        if o.studio_footer {
            let y = plan.page_h as f32 - sheet::mm_px(3.0, dpi) as f32;
            text::draw_centered(&mut page, "Ripon AI Photo Studio", plan.page_w as f32 / 2.0, y, sheet::mm_px(2.2, dpi) as f32, plan.page_w as f32 * 0.8, [150, 150, 150]);
        }
        pages.push(page);
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mm(v: f32) -> u32 {
        sheet::mm_px(v, 300)
    }
    #[test]
    fn ten_customers_two_copies_fit_one_a4() {
        let items = vec![Item { w: mm(35.0), h: mm(45.0), copies: 2 }; 10];
        let o = SheetOpts::default();
        let p = plan(&items, &o, false, 300, true);
        assert_eq!(p.total, 20);
        assert_eq!(p.pages, 1);
        assert_eq!(p.placed.len(), 20);
        assert!(p.too_big.is_empty());
        // nothing overlaps, everything inside the page
        for (i, a) in p.placed.iter().enumerate() {
            assert!(a.x >= 0 && a.y >= 0 && a.x + a.w as i64 <= p.page_w as i64 && a.y + a.h as i64 <= p.page_h as i64);
            for b in &p.placed[i + 1..] {
                let apart = a.x + a.w as i64 <= b.x || b.x + b.w as i64 <= a.x || a.y + a.h as i64 <= b.y || b.y + b.h as i64 <= a.y;
                assert!(apart, "overlap {a:?} {b:?}");
            }
        }
    }
    #[test]
    fn many_copies_use_more_pages_and_each_customer_gets_exact_copies() {
        let items = vec![Item { w: mm(35.0), h: mm(45.0), copies: 3 }, Item { w: mm(40.0), h: mm(50.0), copies: 20 }, Item { w: mm(25.0), h: mm(30.0), copies: 1 }];
        let p = plan(&items, &SheetOpts::default(), false, 300, true);
        assert!(p.pages >= 2);
        for (i, it) in items.iter().enumerate() {
            assert_eq!(p.placed.iter().filter(|x| x.item == i).count() as u32, it.copies);
        }
        let imgs: Vec<RgbImage> = items.iter().map(|it| RgbImage::from_pixel(it.w, it.h, Rgb([10, 20, 30]))).collect();
        let refs: Vec<&RgbImage> = imgs.iter().collect();
        assert_eq!(render(&p, &refs, &SheetOpts::default(), 300).len(), p.pages);
    }
    #[test]
    fn landscape_and_too_big() {
        let items = vec![Item { w: mm(35.0), h: mm(45.0), copies: 4 }, Item { w: mm(250.0), h: mm(300.0), copies: 1 }];
        let p = plan(&items, &SheetOpts::default(), true, 300, true);
        assert!(p.page_w > p.page_h);
        assert_eq!(p.too_big, vec![1]);
        assert_eq!(p.placed.len(), 4);
    }
    #[test]
    fn no_distortion_sizes_kept() {
        let items = vec![Item { w: mm(35.0), h: mm(45.0), copies: 6 }];
        let mut o = SheetOpts::default();
        o.auto_rotate = false;
        let p = plan(&items, &o, false, 300, true);
        assert!(p.placed.iter().all(|x| x.w == mm(35.0) && x.h == mm(45.0) && !x.rotated));
    }
}
