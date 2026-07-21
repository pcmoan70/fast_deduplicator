//! Sharpness scoring: contrast-normalized Tenengrad (Sobel gradient energy)
//! at preview resolution. Scores are comparable across frames of a burst;
//! absolute values are only meaningful relative to each other.
//!
//! Deliberately single-scale: per-scale normalization at downsampled levels
//! systematically scores blurred content HIGHER (decimation re-sharpens the
//! residual spectrum), so a multi-scale mix dilutes discrimination. Verified
//! empirically: full-res normalized Tenengrad separates a 2x-blurred frame
//! by 2:1, the 3-scale mix only by 1.06:1.

use crate::decode::Luma;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sharpness {
    /// Combined multi-scale, contrast-normalized score. Higher = sharper.
    pub score: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// Contrast-normalized Tenengrad over a region: mean Sobel energy divided by
/// luma variance, so exposure flicker within a burst doesn't move scores.
fn tenengrad_norm(px: &[u8], stride: usize, r: Rect) -> f32 {
    if r.w < 8 || r.h < 8 {
        return 0.0;
    }
    let mut energy = 0.0f64;
    let mut sum = 0.0f64;
    let mut sum2 = 0.0f64;
    let mut n = 0.0f64;
    for y in (r.y + 1)..(r.y + r.h - 1) {
        let row = y * stride;
        for x in (r.x + 1)..(r.x + r.w - 1) {
            let i = row + x;
            let a = px[i - stride - 1] as i32;
            let b = px[i - stride] as i32;
            let c = px[i - stride + 1] as i32;
            let d = px[i - 1] as i32;
            let f = px[i + 1] as i32;
            let g = px[i + stride - 1] as i32;
            let h = px[i + stride] as i32;
            let k = px[i + stride + 1] as i32;
            let gx = (c + 2 * f + k) - (a + 2 * d + g);
            let gy = (g + 2 * h + k) - (a + 2 * b + c);
            energy += (gx * gx + gy * gy) as f64;
            let v = px[i] as f64;
            sum += v;
            sum2 += v * v;
            n += 1.0;
        }
    }
    if n == 0.0 {
        return 0.0;
    }
    let mean = sum / n;
    let var = (sum2 / n - mean * mean).max(1.0);
    (energy / n / var) as f32
}

pub fn score_region(luma: &Luma, r: Rect) -> Sharpness {
    let full = Rect {
        x: r.x.min(luma.width.saturating_sub(1)),
        y: r.y.min(luma.height.saturating_sub(1)),
        w: r.w.min(luma.width - r.x.min(luma.width)),
        h: r.h.min(luma.height - r.y.min(luma.height)),
    };
    Sharpness {
        score: tenengrad_norm(&luma.pixels, luma.width, full),
    }
}

/// Global sharpness: max over a coarse tile grid, weighted toward center.
/// Max-over-tiles beats whole-frame average (which rewards busy
/// backgrounds); the winner tile is usually the subject.
pub fn score_global(luma: &Luma) -> Sharpness {
    const TX: usize = 9;
    const TY: usize = 6;
    let tw = luma.width / TX;
    let th = luma.height / TY;
    if tw < 16 || th < 16 {
        return score_region(
            luma,
            Rect {
                x: 0,
                y: 0,
                w: luma.width,
                h: luma.height,
            },
        );
    }
    let mut best = 0.0f32;
    for ty in 0..TY {
        for tx in 0..TX {
            let s = score_region(
                luma,
                Rect {
                    x: tx * tw,
                    y: ty * th,
                    w: tw,
                    h: th,
                },
            );
            // Center weight: 1.0 middle, 0.6 corners.
            let dx = (tx as f32 + 0.5) / TX as f32 - 0.5;
            let dy = (ty as f32 + 0.5) / TY as f32 - 0.5;
            let w = 1.0 - 0.8 * (dx * dx + dy * dy).sqrt();
            best = best.max(s.score * w);
        }
    }
    Sharpness { score: best }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic check: a blurred pattern must score below the sharp one.
    #[test]
    fn blur_is_monotonic() {
        let w: usize = 256;
        let h: usize = 256;
        // Broad-spectrum pattern (LCG noise): single-frequency patterns like
        // checkerboards defeat contrast normalization, real content doesn't.
        let mut state = 0x12345678u32;
        let sharp: Vec<u8> = (0..w * h)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (state >> 24) as u8
            })
            .collect();
        // separable 5-tap box blur, horizontal then vertical, applied twice
        let blur_once = |src: &Vec<u8>| -> Vec<u8> {
            let hpass: Vec<u8> = (0..w * h)
                .map(|i| {
                    let x = i % w;
                    let lo = x.saturating_sub(2);
                    let hi = (x + 2).min(w - 1);
                    let row = i - x;
                    let s: u32 = (lo..=hi).map(|xx| src[row + xx] as u32).sum();
                    (s / (hi - lo + 1) as u32) as u8
                })
                .collect();
            (0..w * h)
                .map(|i| {
                    let y = i / w;
                    let lo = y.saturating_sub(2);
                    let hi = (y + 2).min(h - 1);
                    let s: u32 = (lo..=hi).map(|yy| hpass[yy * w + i % w] as u32).sum();
                    (s / (hi - lo + 1) as u32) as u8
                })
                .collect()
        };
        let blurred = blur_once(&blur_once(&sharp));
        let l1 = Luma {
            pixels: sharp,
            width: w,
            height: h,
        };
        let l2 = Luma {
            pixels: blurred,
            width: w,
            height: h,
        };
        let r = Rect { x: 0, y: 0, w, h };
        let (s1, s2) = (score_region(&l1, r).score, score_region(&l2, r).score);
        assert!(s1 > s2 * 1.5, "sharp={} blurred={}", s1, s2);
        assert!(score_global(&l1).score > score_global(&l2).score);
    }
}
