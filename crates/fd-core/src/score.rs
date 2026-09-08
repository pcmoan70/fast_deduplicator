//! Sharpness scoring: contrast-normalized Laplacian energy at the working
//! resolution (~1620-2048 px). Scores are comparable across frames of a
//! burst; absolute values are only meaningful relative to each other.
//!
//! Why the Laplacian (chosen 2026-09-04 with `examples/sharpbench.rs`, real
//! frames degraded at native scale): a 2 px native focus miss is ~0.5 px
//! after the 4x downsample, and a 3x3 Sobel has no response at that band, so
//! Sobel energy / variance separated it only 1.13-1.22x; the 5-point
//! Laplacian / variance separates it 2.0-3.6x, still ranks sharp above
//! blurred under heavy common-mode noise (1.2-1.4x), and costs the same.
//! Measuring at native resolution instead did not improve the
//! discrimination-to-jitter ratio and inflated noise 3-10x, so it is not
//! done. Deliberately single-scale: per-scale normalization at downsampled
//! levels scores blurred content HIGHER (decimation re-sharpens the
//! residual spectrum), see tasks/lessons.md.

use crate::decode::Luma;

/// Bumped whenever the formula changes so cached scores from an older
/// formula are never mixed with new ones (folded into `cache::file_key`).
pub const SCORE_VERSION: u32 = 2;

/// Tiles whose luma variance is below this (std-dev 20) are scaled down in
/// `score_global`: pure sensor noise has a Laplacian/variance ratio of ~20
/// regardless of amplitude, so without a contrast gate a flat, noisy sky
/// tile would beat the subject in max-over-tiles. Within a burst the gate is
/// common-mode, so rankings are unaffected.
const GATE_VAR: f64 = 400.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sharpness {
    /// Contrast-normalized Laplacian energy (x100). Higher = sharper.
    pub score: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// Mean squared 5-point Laplacian divided by luma variance (x100) over a
/// region, so exposure flicker within a burst doesn't move scores. Also
/// returns the variance for the contrast gate.
fn laplacian_norm(px: &[u8], stride: usize, r: Rect) -> (f32, f64) {
    if r.w < 8 || r.h < 8 {
        return (0.0, 0.0);
    }
    let mut energy = 0.0f64;
    let mut sum = 0.0f64;
    let mut sum2 = 0.0f64;
    let mut n = 0.0f64;
    for y in (r.y + 1)..(r.y + r.h - 1) {
        let row = y * stride;
        for x in (r.x + 1)..(r.x + r.w - 1) {
            let i = row + x;
            let v = px[i] as i32;
            let lap = px[i - 1] as i32 + px[i + 1] as i32 + px[i - stride] as i32
                + px[i + stride] as i32
                - 4 * v;
            energy += (lap * lap) as f64;
            let v = v as f64;
            sum += v;
            sum2 += v * v;
            n += 1.0;
        }
    }
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let mean = sum / n;
    let var = (sum2 / n - mean * mean).max(1.0);
    ((100.0 * energy / n / var) as f32, var)
}

pub fn score_region(luma: &Luma, r: Rect) -> Sharpness {
    let full = Rect {
        x: r.x.min(luma.width.saturating_sub(1)),
        y: r.y.min(luma.height.saturating_sub(1)),
        w: r.w.min(luma.width - r.x.min(luma.width)),
        h: r.h.min(luma.height - r.y.min(luma.height)),
    };
    Sharpness {
        score: laplacian_norm(&luma.pixels, luma.width, full).0,
    }
}

/// Global sharpness: max over a coarse tile grid, weighted toward center
/// and gated by tile contrast (see `GATE_VAR`). Max-over-tiles beats
/// whole-frame average (which rewards busy backgrounds); the winner tile is
/// usually the subject.
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
            let (s, var) = laplacian_norm(
                &luma.pixels,
                luma.width,
                Rect {
                    x: tx * tw,
                    y: ty * th,
                    w: tw,
                    h: th,
                },
            );
            let gate = (var / GATE_VAR).min(1.0) as f32;
            // Center weight: 1.0 middle, 0.6 corners.
            let dx = (tx as f32 + 0.5) / TX as f32 - 0.5;
            let dy = (ty as f32 + 0.5) / TY as f32 - 0.5;
            let w = 1.0 - 0.8 * (dx * dx + dy * dy).sqrt();
            best = best.max(s * gate * w);
        }
    }
    Sharpness { score: best }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(state: &mut u32) -> u32 {
        *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        *state
    }

    /// Box blur of radius `r` (separable, edge-clamped), in f32.
    fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
        let mut tmp = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
                tmp[y * w + x] = (lo..=hi).map(|xx| src[y * w + xx]).sum::<f32>() / (hi - lo + 1) as f32;
            }
        }
        let mut out = vec![0f32; w * h];
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
            for x in 0..w {
                out[y * w + x] = (lo..=hi).map(|yy| tmp[yy * w + x]).sum::<f32>() / (hi - lo + 1) as f32;
            }
        }
        out
    }

    /// Separable Gaussian blur in f32 (radius 3 sigma, edge-clamped).
    fn gauss(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
        let r = (3.0 * sigma).ceil() as isize;
        let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let norm: f32 = k.iter().sum();
        let at = |v: &[f32], x: isize, y: isize| v[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        let mut tmp = vec![0f32; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                tmp[(y * w as isize + x) as usize] =
                    k.iter().enumerate().map(|(j, kv)| kv * at(src, x + j as isize - r, y)).sum::<f32>() / norm;
            }
        }
        let mut out = vec![0f32; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                out[(y * w as isize + x) as usize] =
                    k.iter().enumerate().map(|(j, kv)| kv * at(&tmp, x, y + j as isize - r)).sum::<f32>() / norm;
            }
        }
        out
    }

    /// Natural-looking test content: three octaves of smoothed noise,
    /// stretched to 30..220 so exposure scaling does not clip.
    fn natural(w: usize, h: usize) -> Vec<f32> {
        let mut state = 0xC0FFEEu32;
        let noise: Vec<f32> = (0..w * h).map(|_| (lcg(&mut state) >> 24) as f32).collect();
        let o1 = box_blur(&noise, w, h, 1);
        let o2 = box_blur(&noise, w, h, 4);
        let o3 = box_blur(&noise, w, h, 16);
        let mix: Vec<f32> = (0..w * h).map(|i| o1[i] + 2.0 * o2[i] + 4.0 * o3[i]).collect();
        let (lo, hi) = mix.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        mix.iter().map(|v| 30.0 + 190.0 * (v - lo) / (hi - lo)).collect()
    }

    fn add_noise(src: &[f32], sigma: f32, seed: u32) -> Vec<f32> {
        let mut state = seed;
        src.iter()
            .map(|&v| {
                let g: f32 = (0..12).map(|_| (lcg(&mut state) >> 8) as f32 / (1u32 << 24) as f32).sum::<f32>() - 6.0;
                v + sigma * g
            })
            .collect()
    }

    fn luma(px: &[f32], w: usize, h: usize) -> Luma {
        Luma {
            pixels: px.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect(),
            width: w,
            height: h,
        }
    }

    fn s(px: &[f32], w: usize, h: usize) -> f32 {
        score_region(&luma(px, w, h), Rect { x: 0, y: 0, w, h }).score
    }

    /// Sub-pixel blur at working resolution (0.5 and 1 px ~ 2 and 4 px
    /// native) must be separated clearly and in order.
    #[test]
    fn fine_blur_is_discriminated() {
        let (w, h) = (256, 256);
        let sharp = natural(w, h);
        let (b05, b1) = (gauss(&sharp, w, h, 0.5), gauss(&sharp, w, h, 1.0));
        let (s0, s05, s1) = (s(&sharp, w, h), s(&b05, w, h), s(&b1, w, h));
        assert!(s0 > s05 && s05 > s1, "sharp={s0} blur0.5={s05} blur1={s1}");
        assert!(s0 / s05 > 1.3, "blur 0.5 px separated only {}x", s0 / s05);
        assert!(s0 / s1 > 2.0, "blur 1 px separated only {}x", s0 / s1);
    }

    /// Common-mode sensor noise must not rank a blurred frame above a sharp
    /// one, and must not blow the score up.
    #[test]
    fn noise_does_not_invert_ranking() {
        let (w, h) = (256, 256);
        let sharp = natural(w, h);
        let blurred = gauss(&sharp, w, h, 1.0);
        let (sn, bn) = (add_noise(&sharp, 2.0, 5), add_noise(&blurred, 2.0, 5));
        assert!(s(&sn, w, h) > s(&bn, w, h));
        assert!(s(&sn, w, h) / s(&sharp, w, h) < 2.0);
    }

    #[test]
    fn exposure_flicker_is_invisible() {
        let (w, h) = (256, 256);
        let sharp = natural(w, h);
        let dim: Vec<f32> = sharp.iter().map(|v| v * 0.8).collect();
        let (a, b) = (s(&sharp, w, h), s(&dim, w, h));
        assert!((a - b).abs() < 0.05 * a, "exposure moved the score {a} -> {b}");
    }

    /// A flat, noisy tile (sky) must not win max-over-tiles.
    #[test]
    fn flat_noise_scores_low() {
        let (w, h) = (256, 256);
        let natural_global = score_global(&luma(&natural(w, h), w, h)).score;
        let flat = add_noise(&vec![128.0; w * h], 2.0, 9);
        let flat_global = score_global(&luma(&flat, w, h)).score;
        assert!(flat_global < 0.5 * natural_global, "flat={flat_global} natural={natural_global}");
    }

    /// Coarse check kept from the first version: a heavily blurred pattern
    /// must score well below the sharp one, also through `score_global`.
    #[test]
    fn coarse_blur_is_monotonic() {
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
