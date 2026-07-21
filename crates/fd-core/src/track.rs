//! Point tracking through a burst: pyramidal normalized cross-correlation
//! template matching. Works on preview-resolution luma (~1620px). Dual
//! template (original + slowly updated) resists drift; per-frame confidence
//! is the NCC peak value.

use crate::decode::Luma;

pub const TEMPLATE: usize = 64; // template side at preview resolution
pub const SEARCH: usize = 96; // search radius around previous position

#[derive(Clone)]
pub struct Template {
    px: Vec<f32>, // zero-mean, unit-norm
    side: usize,
}

fn extract_patch(l: &Luma, cx: f32, cy: f32, side: usize) -> Option<Vec<f32>> {
    let half = side as f32 / 2.0;
    let x0 = (cx - half).round() as i64;
    let y0 = (cy - half).round() as i64;
    if x0 < 0 || y0 < 0 || x0 + side as i64 > l.width as i64 || y0 + side as i64 > l.height as i64
    {
        return None;
    }
    let mut out = Vec::with_capacity(side * side);
    for y in 0..side {
        let row = (y0 as usize + y) * l.width + x0 as usize;
        for x in 0..side {
            out.push(l.pixels[row + x] as f32);
        }
    }
    Some(out)
}

fn normalize(mut v: Vec<f32>) -> Option<Vec<f32>> {
    let n = v.len() as f32;
    let mean = v.iter().sum::<f32>() / n;
    let mut ss = 0.0;
    for p in v.iter_mut() {
        *p -= mean;
        ss += *p * *p;
    }
    if ss < 1e-3 {
        return None; // flat patch, untrackable
    }
    let inv = ss.sqrt().recip();
    for p in v.iter_mut() {
        *p *= inv;
    }
    Some(v)
}

impl Template {
    pub fn from(l: &Luma, cx: f32, cy: f32, side: usize) -> Option<Template> {
        let px = normalize(extract_patch(l, cx, cy, side)?)?;
        Some(Template { px, side })
    }
}

/// NCC of a template against the patch at (cx, cy). Returns -1..1.
fn ncc_at(l: &Luma, t: &Template, cx: f32, cy: f32) -> f32 {
    let Some(patch) = extract_patch(l, cx, cy, t.side) else {
        return -1.0;
    };
    let Some(patch) = normalize(patch) else {
        return -1.0;
    };
    t.px.iter().zip(&patch).map(|(a, b)| a * b).sum()
}

/// Coarse-to-fine search around `prev` within +-SEARCH. Returns position and
/// confidence (NCC peak).
fn search(l: &Luma, t: &Template, prev: (f32, f32)) -> ((f32, f32), f32) {
    fn probe(l: &Luma, t: &Template, x: f32, y: f32, best: &mut ((f32, f32), f32)) {
        let v = ncc_at(l, t, x, y);
        if v > best.1 {
            *best = ((x, y), v);
        }
    }
    // Coarse: stride 8 over the window
    let mut best = (prev, -1.0f32);
    let s = SEARCH as i32;
    let mut y = -s;
    while y <= s {
        let mut x = -s;
        while x <= s {
            probe(l, t, prev.0 + x as f32, prev.1 + y as f32, &mut best);
            x += 8;
        }
        y += 8;
    }
    // Refine: stride 2 then 1 around the coarse peak
    for stride in [2i32, 1] {
        let center = best.0;
        let r = stride * 4;
        let mut y = -r;
        while y <= r {
            let mut x = -r;
            while x <= r {
                probe(
                    l,
                    t,
                    center.0 + x as f32,
                    center.1 + y as f32,
                    &mut best,
                );
                x += stride;
            }
            y += stride;
        }
    }
    best
}

pub struct Tracker {
    original: Template,
    current: Template,
    pub pos: (f32, f32),
}

impl Tracker {
    pub fn new(seed: &Luma, x: f32, y: f32) -> Option<Tracker> {
        let t = Template::from(seed, x, y, TEMPLATE)?;
        Some(Tracker {
            original: t.clone(),
            current: t,
            pos: (x, y),
        })
    }

    /// Advance to the next frame; returns (x, y, confidence).
    pub fn step(&mut self, l: &Luma) -> (f32, f32, f32) {
        let (p_orig, c_orig) = search(l, &self.original, self.pos);
        let (p_cur, c_cur) = search(l, &self.current, self.pos);
        let (pos, conf) = if c_orig >= c_cur {
            (p_orig, c_orig)
        } else {
            (p_cur, c_cur)
        };
        if conf >= 0.55 {
            self.pos = pos;
            // Refresh the adaptive template only on confident matches.
            if conf >= 0.8 {
                if let Some(t) = Template::from(l, pos.0, pos.1, TEMPLATE) {
                    self.current = t;
                }
            }
        }
        (self.pos.0, self.pos.1, conf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoothed noise: real images have spatial autocorrelation; the coarse
    /// stride-8 search relies on it (pure white noise has none).
    fn noise_image(w: usize, h: usize, seed: u32) -> Luma {
        let mut state = seed;
        let mut px: Vec<u8> = (0..w * h)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (state >> 24) as u8
            })
            .collect();
        for _ in 0..2 {
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0u32;
                    let mut n = 0u32;
                    for dy in -3i32..=3 {
                        for dx in -3i32..=3 {
                            let (sx, sy) = (x as i32 + dx, y as i32 + dy);
                            if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                                sum += px[sy as usize * w + sx as usize] as u32;
                                n += 1;
                            }
                        }
                    }
                    out[y * w + x] = (sum / n) as u8;
                }
            }
            px = out;
        }
        // Re-stretch contrast after smoothing so patches aren't near-flat.
        let (mut lo, mut hi) = (255u8, 0u8);
        for &p in &px {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let span = (hi - lo).max(1) as u32;
        for p in px.iter_mut() {
            *p = ((*p as u32 - lo as u32) * 255 / span) as u8;
        }
        Luma {
            pixels: px,
            width: w,
            height: h,
        }
    }

    fn shift(l: &Luma, dx: i32, dy: i32) -> Luma {
        let mut out = vec![128u8; l.pixels.len()];
        for y in 0..l.height as i32 {
            for x in 0..l.width as i32 {
                let (sx, sy) = (x - dx, y - dy);
                if sx >= 0 && sy >= 0 && (sx as usize) < l.width && (sy as usize) < l.height {
                    out[y as usize * l.width + x as usize] =
                        l.pixels[sy as usize * l.width + sx as usize];
                }
            }
        }
        Luma {
            pixels: out,
            width: l.width,
            height: l.height,
        }
    }

    #[test]
    fn tracks_translation() {
        let base = noise_image(400, 300, 7);
        let mut tr = Tracker::new(&base, 200.0, 150.0).unwrap();
        // Move the scene in steps; tracker must follow with high confidence.
        let mut total = (0i32, 0i32);
        for (dx, dy) in [(15, -9), (30, 4), (42, 20)] {
            let moved = shift(&base, dx, dy);
            let (x, y, conf) = tr.step(&moved);
            total = (dx, dy);
            assert!(conf > 0.8, "conf {conf}");
            assert!(
                (x - (200.0 + total.0 as f32)).abs() <= 1.5
                    && (y - (150.0 + total.1 as f32)).abs() <= 1.5,
                "pos ({x},{y}) expected ({},{})",
                200 + total.0,
                150 + total.1
            );
        }
    }

    #[test]
    fn lost_track_has_low_confidence() {
        let base = noise_image(400, 300, 7);
        let unrelated = noise_image(400, 300, 999);
        let mut tr = Tracker::new(&base, 200.0, 150.0).unwrap();
        let (_, _, conf) = tr.step(&unrelated);
        assert!(conf < 0.5, "conf {conf} should be low on unrelated frame");
    }
}
