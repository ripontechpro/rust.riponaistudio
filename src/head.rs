//! Head position from the person silhouette (no face-detection model needed).
//! Finds crown, widest part of the head and the neck, then estimates the chin.
use image::{imageops, GrayImage};

#[derive(Clone, Copy, Debug)]
pub struct Head {
    pub cx: f32,
    pub crown_y: f32,
    pub chin_y: f32,
    pub width: f32,
}

/// Coordinates returned are in the pixel space of `alpha`.
pub fn estimate(alpha: &GrayImage) -> Option<Head> {
    let (aw, ah) = alpha.dimensions();
    let k = (256.0 / aw.max(ah) as f32).min(1.0);
    let (w, h) = (((aw as f32 * k) as u32).max(8), ((ah as f32 * k) as u32).max(8));
    let small = imageops::resize(alpha, w, h, imageops::FilterType::Triangle);
    let (w, h) = (w as usize, h as usize);
    let mut mask: Vec<bool> = small.pixels().map(|p| p[0] > 128).collect();

    // keep only the biggest blob (the person)
    let mut label = vec![0u32; w * h];
    let mut best = (0usize, 0u32);
    let mut next = 1u32;
    let mut stack = Vec::new();
    for s in 0..w * h {
        if !mask[s] || label[s] != 0 {
            continue;
        }
        let mut count = 0usize;
        stack.push(s);
        label[s] = next;
        while let Some(i) = stack.pop() {
            count += 1;
            let (x, y) = (i % w, i / w);
            let mut push = |j: usize| {
                if mask[j] && label[j] == 0 {
                    label[j] = next;
                    stack.push(j);
                }
            };
            if x > 0 { push(i - 1); }
            if x + 1 < w { push(i + 1); }
            if y > 0 { push(i - w); }
            if y + 1 < h { push(i + w); }
        }
        if count > best.0 {
            best = (count, next);
        }
        next += 1;
    }
    if best.0 < (w * h) / 80 {
        return None; // nothing person-sized
    }
    for i in 0..w * h {
        mask[i] = label[i] == best.1;
    }

    // row widths
    let mut rw = vec![0f32; h];
    for y in 0..h {
        rw[y] = mask[y * w..(y + 1) * w].iter().filter(|&&m| m).count() as f32;
    }
    let sm: Vec<f32> = (0..h)
        .map(|y| {
            let lo = y.saturating_sub(1);
            let hi = (y + 1).min(h - 1);
            rw[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32
        })
        .collect();
    let thr = (0.03 * w as f32).max(3.0);
    let crown = (0..h).find(|&y| sm[y] >= thr)?;
    let bottom = (0..h).rev().find(|&y| sm[y] >= 2.0).unwrap_or(h - 1);
    let hp = (bottom - crown).max(1) as f32;

    // widest part of the head = first strong local maximum
    let mut max_w = 0f32;
    let mut y_max = crown;
    let mut y = crown;
    let limit = (crown as f32 + 0.7 * hp) as usize;
    while y < limit.min(h - 1) {
        if sm[y] > max_w {
            max_w = sm[y];
            y_max = y;
        }
        if max_w > 0.06 * w as f32 && sm[y] < 0.86 * max_w {
            break;
        }
        y += 1;
    }
    // neck = narrowest row after the head, before the shoulders start to widen again
    let mut neck: Option<usize> = None;
    {
        let mut min_w = f32::MAX;
        let mut y_min = y_max;
        for y in y_max..=bottom.min(h - 1) {
            if sm[y] < min_w {
                min_w = sm[y];
                y_min = y;
            }
            if sm[y] > min_w * 1.35 && y > y_min + 1 && min_w < 0.95 * max_w {
                neck = Some(y_min);
                break;
            }
        }
    }
    let head_w = max_w.max(4.0);
    let h_from_w = head_w / 0.70;
    let head_h = match neck {
        Some(n) if n > crown + 4 => {
            let h_neck = (n - crown) as f32 * 0.89;
            let est = 0.6 * h_neck + 0.4 * h_from_w;
            est.clamp(0.7 * h_from_w, 1.3 * h_from_w)
        }
        _ => h_from_w,
    };
    // centre x of the upper head
    let y_end = ((crown as f32 + 0.5 * head_h) as usize).min(h - 1);
    let (mut sx, mut n) = (0f64, 0f64);
    for y in crown..=y_end {
        for x in 0..w {
            if mask[y * w + x] {
                sx += x as f64;
                n += 1.0;
            }
        }
    }
    if n < 1.0 {
        return None;
    }
    let inv = 1.0 / k;
    Some(Head {
        cx: (sx / n) as f32 * inv + 0.5 * inv,
        crown_y: crown as f32 * inv,
        chin_y: (crown as f32 + head_h) * inv,
        width: head_w * inv,
    })
}
