//! Eye sharpness: segment the pupil (or the whole dark eye) inside a search
//! window at native resolution, then measure the 10-90% rise width of the
//! luma transition across its rim along many rays. The width in pixels is
//! the physical blur at the eye, which is what a viewer means by "the eye is
//! sharp": defocus widens every ray, motion blur widens the rays across the
//! motion, and neither content nor contrast enters the number.

use crate::decode::Luma;
use crate::score::Rect;

/// A dark blob as an ellipse: centre, semi-axes (`a >= b`), angle of `a`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blob {
    pub cx: f32,
    pub cy: f32,
    pub a: f32,
    pub b: f32,
    pub phi: f32,
    pub area: usize,
}

/// Rim transition widths in native pixels over the valid rays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeStats {
    /// 75th percentile: what the eye reads as the blur (see module doc).
    pub width_px: f32,
    pub median_px: f32,
    /// p90 / p50; above ~1.3 the blur is directional (motion).
    pub anisotropy: f32,
    pub rays: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeAcuity {
    pub blob: Blob,
    pub edge: EdgeStats,
}

/// Focus-peaking threshold in luma levels per pixel: the gradient magnitude
/// of the 3x3-smoothed luma above this counts as "in focus" (a 100-level
/// edge 5 px wide is ~20/px; the same edge blurred to 15 px is ~7/px). The
/// display overlay paints exactly the pixels that `coverage` counts.
pub const PEAK_THRESHOLD: f32 = 8.0;

/// 3x3 box-smoothed copy of an 8-bit plane (edge-clamped), two passes.
fn smooth3(px: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut tmp = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(1), (x + 1).min(w - 1));
            tmp[y * w + x] = px[y * w + a] as u16 + px[y * w + x] as u16 + px[y * w + b] as u16;
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            out[y * w + x] = ((tmp[a * w + x] + tmp[y * w + x] + tmp[b * w + x]) / 9) as u8;
        }
    }
    out
}

/// Which pixels of `r` pass the peaking test (row-major, `r.w` x `r.h`).
pub fn peaking_mask(px: &[u8], w: usize, h: usize, r: Rect) -> Vec<bool> {
    let sm = smooth3(px, w, h);
    let mut mask = vec![false; r.w * r.h];
    for y in r.y..(r.y + r.h).min(h) {
        if y == 0 || y + 1 >= h {
            continue;
        }
        for x in r.x..(r.x + r.w).min(w) {
            if x == 0 || x + 1 >= w {
                continue;
            }
            let i = y * w + x;
            let gx = (sm[i + 1] as f32 - sm[i - 1] as f32) * 0.5;
            let gy = (sm[i + w] as f32 - sm[i - w] as f32) * 0.5;
            mask[(y - r.y) * r.w + (x - r.x)] = gx * gx + gy * gy > PEAK_THRESHOLD * PEAK_THRESHOLD;
        }
    }
    mask
}

/// Fraction of `r` that passes the peaking test: how much of the focus area
/// is in focus, 0..1.
pub fn coverage(l: &Luma, r: Rect) -> f32 {
    let r = clamp_rect(l, r);
    let m = peaking_mask(&l.pixels, l.width, l.height, r);
    m.iter().filter(|&&b| b).count() as f32 / m.len().max(1) as f32
}

/// Minimum valid rays out of `RAYS` for a measurement to count.
const RAYS: usize = 64;
const MIN_RAYS: usize = 16;
/// Minimum luma step across the rim for a ray to count.
const MIN_STEP: f32 = 20.0;
/// Smallest blob accepted as a pupil (radius 6 px).
const MIN_AREA: usize = 113;

/// Segment and measure the eye in `search`; `max_r` is the largest pupil
/// radius that makes sense there (half the eye box), so the dark eye-ring of
/// a pale-eyed bird is never taken for the pupil. Falls back to a nominal
/// circle at the search centre when no blob is found (`area == 0`), and
/// lets the rays decide.
pub fn measure_eye(l: &Luma, search: Rect, max_r: f32) -> Option<EyeAcuity> {
    let blob = segment_dark_blob(l, search, max_r).unwrap_or(Blob {
        cx: search.x as f32 + search.w as f32 / 2.0,
        cy: search.y as f32 + search.h as f32 / 2.0,
        a: 0.3 * search.w.min(search.h) as f32,
        b: 0.3 * search.w.min(search.h) as f32,
        phi: 0.0,
        area: 0,
    });
    let edge = edge_width(l, &blob, RAYS)?;
    Some(EyeAcuity { blob, edge })
}

fn clamp_rect(l: &Luma, r: Rect) -> Rect {
    let x = r.x.min(l.width.saturating_sub(1));
    let y = r.y.min(l.height.saturating_sub(1));
    Rect { x, y, w: r.w.min(l.width - x), h: r.h.min(l.height - y) }
}

/// The pupil as the darkest compact blob near the centre of `search`.
/// Seed = darkest (3x3-smoothed) pixel within a quarter of the window from
/// its centre. The region is grown from the seed at rising thresholds and
/// its area recorded: the pupil is the first plateau where the area barely
/// changes over several levels (a soft rim only adds a few percent per level,
/// a merge with the iris or eye-ring multiplies it). Fixed percentiles fail
/// here because the pupil is a few percent of the window on a dark-eyed owl
/// and a fraction of a percent on a pale-eyed bird.
pub fn segment_dark_blob(l: &Luma, search: Rect, max_r: f32) -> Option<Blob> {
    let r = clamp_rect(l, search);
    if r.w < 16 || r.h < 16 {
        return None;
    }
    // Segment on a 3x3-smoothed copy so speckle noise cannot fragment the
    // region sweep; the rays are measured on the original pixels.
    let (w, h) = (r.w, r.h);
    let src = |x: usize, y: usize| l.pixels[(r.y + y) * l.width + r.x + x] as u32;
    let sm: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
            let mut sum = 0;
            let mut n = 0;
            for yy in y0..=y1 {
                for xx in x0..=x1 {
                    sum += src(xx, yy);
                    n += 1;
                }
            }
            (sum / n) as u8
        })
        .collect();
    let at = |x: usize, y: usize| sm[y * w + x] as u32;
    // Median of the window bounds the threshold sweep.
    let mut hist = [0u32; 256];
    for &v in &sm {
        hist[v as usize] += 1;
    }
    let half = (w * h / 2) as u32;
    let mut acc = 0;
    let med = hist.iter().position(|&c| { acc += c; acc >= half }).unwrap_or(255) as u32;
    let (cx0, cy0) = (w as f32 / 2.0, h as f32 / 2.0);
    let reach = 0.25 * w.min(h) as f32;
    // Up to three seeds: the darkest pixel near the centre (mild distance
    // penalty, 0.3 levels per px), then the darkest outside any region that
    // turned out not to be a pupil (an eye-ring arc, a merged iris).
    let mut excluded = vec![false; w * h];
    for _ in 0..5 {
        let mut seed: Option<(usize, usize, u32, f32)> = None;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let d = (x as f32 - cx0).hypot(y as f32 - cy0);
                if d > reach || excluded[y * w + x] {
                    continue;
                }
                let cost = at(x, y) as f32 + 0.3 * d;
                if seed.is_none_or(|s| cost < s.3) {
                    seed = Some((x, y, at(x, y), cost));
                }
            }
        }
        let Some((sx, sy, dark, _)) = seed else { break };
        excluded[sy * w + sx] = true;
        if med <= dark + 12 {
            break;
        }
        // Area as a function of threshold. The pupil is what exists before
        // the first big jump (merging with the iris or eye-ring); within that
        // range a soft rim only adds a few percent per level, so the middle
        // threshold sits on the rim's 50% level and the ellipse hugs the edge.
        const STEP: u32 = 4;
        let thresholds: Vec<u8> = (1..).map(|i| dark + i * STEP).take_while(|&t| t <= med).map(|t| t as u8).collect();
        let mut areas = Vec::new();
        for &t in &thresholds {
            match grow(&sm, w, h, sx, sy, t) {
                Some((_, a)) => areas.push(a),
                None => break,
            }
        }
        // Whatever this seed grew into is not the pupil unless the fit below
        // says so; exclude it so the next seed lands somewhere else.
        let exclude_region = |excluded: &mut Vec<bool>| {
            if let Some(&t) = thresholds.get(areas.len().saturating_sub(1)) {
                if let Some((comp, _)) = grow(&sm, w, h, sx, sy, t) {
                    for i in 0..w * h {
                        excluded[i] |= comp[i];
                    }
                }
            }
        };
        let Some(first) = areas.iter().position(|&a| a >= MIN_AREA) else {
            exclude_region(&mut excluded);
            continue;
        };
        // A merge adds far more than a 3 px ring around the current blob in
        // one step; a soft rim adds well under one ring.
        let ring = |a: usize| 3.0 * 2.0 * (std::f32::consts::PI * a as f32).sqrt();
        let jump_end = (first..areas.len().saturating_sub(1))
            .find(|&i| (areas[i + 1] - areas[i]) as f32 > ring(areas[i]))
            .map(|i| i + 1)
            .unwrap_or(areas.len());
        // Nothing larger than the eye box can be the pupil.
        let too_big = (first..areas.len())
            .find(|&i| (areas[i] as f32 / std::f32::consts::PI).sqrt() > max_r)
            .unwrap_or(areas.len());
        let end = jump_end.min(too_big);
        if end <= first {
            exclude_region(&mut excluded);
            continue;
        }
        let Some((comp, _)) = grow(&sm, w, h, sx, sy, thresholds[(first + end - 1) / 2]) else { continue };
        if let Some(b) = fit_blob(r, &comp, max_r) {
            return Some(b);
        }
        exclude_region(&mut excluded);
    }
    None
}

/// 4-connected region of pixels `< t` containing the seed, with the holes
/// (catchlight) filled. None if it touches the window border or exceeds half
/// the window. Returns the mask and its area.
fn grow(sm: &[u8], w: usize, h: usize, sx: usize, sy: usize, t: u8) -> Option<(Vec<bool>, usize)> {
    let at = |x: usize, y: usize| sm[y * w + x];
    if at(sx, sy) >= t {
        return None;
    }
    let mut comp = vec![false; w * h];
    let mut stack = vec![(sx, sy)];
    comp[sy * w + sx] = true;
    let mut area = 1usize;
    while let Some((x, y)) = stack.pop() {
        if x == 0 || y == 0 || x == w - 1 || y == h - 1 || area > w * h / 2 {
            return None;
        }
        for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
            if !comp[ny * w + nx] && at(nx, ny) < t {
                comp[ny * w + nx] = true;
                area += 1;
                stack.push((nx, ny));
            }
        }
    }
    // Holes: background not reachable from the border belongs to the blob.
    let mut outside = vec![false; w * h];
    let mut stack: Vec<(usize, usize)> = (0..w)
        .flat_map(|x| [(x, 0), (x, h - 1)])
        .chain((0..h).flat_map(|y| [(0, y), (w - 1, y)]))
        .filter(|&(x, y)| !comp[y * w + x])
        .collect();
    for &(x, y) in &stack {
        outside[y * w + x] = true;
    }
    while let Some((x, y)) = stack.pop() {
        let mut push = |nx: usize, ny: usize| {
            if !outside[ny * w + nx] && !comp[ny * w + nx] {
                outside[ny * w + nx] = true;
                stack.push((nx, ny));
            }
        };
        if x > 0 { push(x - 1, y); }
        if x + 1 < w { push(x + 1, y); }
        if y > 0 { push(x, y - 1); }
        if y + 1 < h { push(x, y + 1); }
    }
    for i in 0..w * h {
        if !outside[i] && !comp[i] {
            comp[i] = true;
            area += 1;
        }
    }
    Some((comp, area))
}

/// Ellipse from the mask's moments, gated on plausibility: big enough, not
/// too elongated, no larger than the eye box, and solid (an arc of eye-ring
/// has an ellipse far larger than its area).
fn fit_blob(r: Rect, comp: &[bool], max_r: f32) -> Option<Blob> {
    let (w, h) = (r.w, r.h);
    let (mut n, mut mx, mut my) = (0f64, 0f64, 0f64);
    for y in 0..h {
        for x in 0..w {
            if comp[y * w + x] {
                n += 1.0;
                mx += x as f64 + 0.5;
                my += y as f64 + 0.5;
            }
        }
    }
    if n == 0.0 {
        return None;
    }
    let (cx, cy) = (mx / n, my / n);
    let (mut sxx, mut syy, mut sxy) = (0f64, 0f64, 0f64);
    for y in 0..h {
        for x in 0..w {
            if comp[y * w + x] {
                let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
                sxx += dx * dx;
                syy += dy * dy;
                sxy += dx * dy;
            }
        }
    }
    let (sxx, syy, sxy) = (sxx / n, syy / n, sxy / n);
    let tr = sxx + syy;
    let det = ((sxx - syy) * (sxx - syy) / 4.0 + sxy * sxy).sqrt();
    let (l1, l2) = (tr / 2.0 + det, (tr / 2.0 - det).max(1e-6));
    // A uniform ellipse has second moments a^2/4 and b^2/4.
    let (a, b) = (2.0 * l1.sqrt(), 2.0 * l2.sqrt());
    let phi = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let area = n as usize;
    let solidity = n / (std::f64::consts::PI * a * b);
    if area < MIN_AREA || a / b > 2.5 || (a * b).sqrt() > max_r as f64 || solidity < 0.6 {
        return None;
    }
    Some(Blob {
        cx: (r.x as f64 + cx) as f32,
        cy: (r.y as f64 + cy) as f32,
        a: a as f32,
        b: b as f32,
        phi: phi as f32,
        area,
    })
}

fn bilinear(l: &Luma, x: f32, y: f32) -> Option<f32> {
    if x < 0.0 || y < 0.0 || x >= (l.width - 1) as f32 || y >= (l.height - 1) as f32 {
        return None;
    }
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let p = |xx: usize, yy: usize| l.pixels[yy * l.width + xx] as f32;
    Some(
        p(x0, y0) * (1.0 - fx) * (1.0 - fy)
            + p(x0 + 1, y0) * fx * (1.0 - fy)
            + p(x0, y0 + 1) * (1.0 - fx) * fy
            + p(x0 + 1, y0 + 1) * fx * fy,
    )
}

/// 10-90% rise width of the dark-inside -> bright-outside transition along
/// one profile (samples `STEP` px apart, inside first). None if the profile
/// has no clean positive step.
fn profile_width(s: &[f32], step: f32) -> Option<f32> {
    let m = s.len();
    if m < 7 {
        return None;
    }
    // [1,2,1] smoothing along the ray only.
    let q: Vec<f32> = (0..m)
        .map(|i| if i == 0 || i == m - 1 { s[i] } else { (s[i - 1] + 2.0 * s[i] + s[i + 1]) / 4.0 })
        .collect();
    let g: Vec<f32> = (0..m).map(|i| if i == 0 || i == m - 1 { 0.0 } else { q[i + 1] - q[i - 1] }).collect();
    let (mut peak, mut gp) = (0usize, 0.0f32);
    for i in 2..m - 2 {
        if g[i] > gp {
            gp = g[i];
            peak = i;
        }
    }
    if gp <= 0.0 {
        return None;
    }
    // Plateaus: walk out while the gradient stays above 10% of the peak.
    let mut j = peak;
    while j > 1 && g[j] > 0.1 * gp {
        j -= 1;
    }
    let lo = (q[j.saturating_sub(2)] + q[j.saturating_sub(1)] + q[j]) / 3.0;
    let mut k = peak;
    while k < m - 2 && g[k] > 0.1 * gp {
        k += 1;
    }
    let hi = (q[k] + q[(k + 1).min(m - 1)] + q[(k + 2).min(m - 1)]) / 3.0;
    let step_h = hi - lo;
    if step_h < MIN_STEP {
        return None;
    }
    let (t10, t90) = (lo + 0.1 * step_h, lo + 0.9 * step_h);
    // Crossings, interpolated, scanning from the peak.
    let mut i = peak;
    while i > 0 && q[i] > t10 {
        i -= 1;
    }
    let x10 = if q[i + 1] != q[i] { i as f32 + (t10 - q[i]) / (q[i + 1] - q[i]) } else { i as f32 };
    let mut i = peak;
    while i + 1 < m && q[i] < t90 {
        i += 1;
    }
    let x90 = if i > 0 && q[i] != q[i - 1] { (i - 1) as f32 + (t90 - q[i - 1]) / (q[i] - q[i - 1]) } else { i as f32 };
    let width = (x90 - x10) * step;
    (width > 0.0 && width < (m as f32) * step).then_some(width)
}

/// Edge widths across the blob's rim along `n_rays` outward normals.
pub fn edge_width(l: &Luma, blob: &Blob, n_rays: usize) -> Option<EdgeStats> {
    const STEP: f32 = 0.5;
    let half = (0.6 * (blob.a * blob.b).sqrt()).clamp(6.0, 24.0);
    let m = (2.0 * half / STEP) as usize + 1;
    let (cphi, sphi) = (blob.phi.cos(), blob.phi.sin());
    let mut widths = Vec::with_capacity(n_rays);
    for k in 0..n_rays {
        let psi = std::f32::consts::TAU * k as f32 / n_rays as f32;
        let (lx, ly) = (blob.a * psi.cos(), blob.b * psi.sin());
        let (px, py) = (blob.cx + cphi * lx - sphi * ly, blob.cy + sphi * lx + cphi * ly);
        // Outward normal of the ellipse at psi, rotated into image space.
        let (nx0, ny0) = (psi.cos() / blob.a, psi.sin() / blob.b);
        let nn = nx0.hypot(ny0);
        let (nx, ny) = ((cphi * nx0 - sphi * ny0) / nn, (sphi * nx0 + cphi * ny0) / nn);
        let samples: Option<Vec<f32>> = (0..m)
            .map(|i| {
                let d = -half + i as f32 * STEP;
                bilinear(l, px + d * nx, py + d * ny)
            })
            .collect();
        if let Some(w) = samples.and_then(|s| profile_width(&s, STEP)) {
            widths.push(w);
        }
    }
    if widths.len() < MIN_RAYS {
        return None;
    }
    widths.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f32| widths[((widths.len() - 1) as f32 * p).round() as usize];
    let (p50, p75, p90) = (pct(0.5), pct(0.75), pct(0.9));
    Some(EdgeStats { width_px: p75, median_px: p50, anisotropy: p90 / p50.max(1e-3), rays: widths.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gauss(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
        if sigma <= 0.0 {
            return src.to_vec();
        }
        let r = (3.0 * sigma).ceil() as isize;
        let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let norm: f32 = k.iter().sum();
        let at = |v: &[f32], x: isize, y: isize| v[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        let mut tmp = vec![0f32; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                tmp[(y * w as isize + x) as usize] = k.iter().enumerate().map(|(j, kv)| kv * at(src, x + j as isize - r, y)).sum::<f32>() / norm;
            }
        }
        let mut out = vec![0f32; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                out[(y * w as isize + x) as usize] = k.iter().enumerate().map(|(j, kv)| kv * at(&tmp, x, y + j as isize - r)).sum::<f32>() / norm;
            }
        }
        out
    }

    /// Horizontal box blur of `len` px (1-D motion).
    fn motion(src: &[f32], w: usize, h: usize, len: usize) -> Vec<f32> {
        let mut out = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let lo = x.saturating_sub(len / 2);
                let hi = (x + len / 2).min(w - 1);
                out[y * w + x] = (lo..=hi).map(|xx| src[y * w + xx]).sum::<f32>() / (hi - lo + 1) as f32;
            }
        }
        out
    }

    /// Background 140, dark disc (25) of radius `r` at `c`, catchlight (250) r=3 inside.
    fn eye_image(w: usize, h: usize, c: (f32, f32), r: f32) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let d = (x - c.0).hypot(y - c.1);
                if (x - (c.0 - 10.0)).hypot(y - (c.1 - 10.0)) < 3.0 {
                    250.0
                } else if d < r {
                    25.0
                } else {
                    140.0
                }
            })
            .collect()
    }

    fn luma(px: &[f32], w: usize, h: usize) -> Luma {
        Luma { pixels: px.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect(), width: w, height: h }
    }

    fn noise(src: &[f32], sigma: f32, seed: u32) -> Vec<f32> {
        let mut state = seed;
        src.iter()
            .map(|&v| {
                let g: f32 = (0..12)
                    .map(|_| {
                        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                        (state >> 8) as f32 / (1u32 << 24) as f32
                    })
                    .sum::<f32>()
                    - 6.0;
                v + sigma * g
            })
            .collect()
    }

    const W: usize = 300;
    const H: usize = 300;
    const ALL: Rect = Rect { x: 0, y: 0, w: W, h: H };

    #[test]
    fn segments_disc_with_catchlight_filled() {
        let img = gauss(&eye_image(W, H, (150.0, 150.0), 35.0), W, H, 1.0);
        let b = segment_dark_blob(&luma(&img, W, H), ALL, 100.0).expect("blob");
        assert!((b.cx - 150.0).abs() < 1.0 && (b.cy - 150.0).abs() < 1.0, "{b:?}");
        let r = (b.a * b.b).sqrt();
        assert!((r - 35.0).abs() < 0.05 * 35.0, "radius {r}");
        let disc = std::f32::consts::PI * 35.0 * 35.0;
        assert!((b.area as f32 - disc).abs() < 0.06 * disc, "area {} (hole not filled?)", b.area);
    }

    #[test]
    fn border_touching_blob_is_rejected() {
        let img = eye_image(W, H, (20.0, 150.0), 35.0);
        assert!(segment_dark_blob(&luma(&img, W, H), ALL, 100.0).is_none());
    }

    #[test]
    fn widths_grow_with_blur_and_match_gaussian_rise() {
        let base = eye_image(W, H, (150.0, 150.0), 35.0);
        let mut last = 0.0;
        for sigma in [0.5f32, 1.0, 1.5, 2.0, 3.0, 4.0] {
            let l = luma(&gauss(&base, W, H, sigma), W, H);
            let m = measure_eye(&l, ALL, 100.0).expect("measure");
            assert!(m.edge.width_px > last, "sigma {sigma}: {} <= {last}", m.edge.width_px);
            last = m.edge.width_px;
            if sigma >= 1.0 {
                let ratio = m.edge.width_px / (2.563 * sigma);
                assert!((0.85..=1.25).contains(&ratio), "sigma {sigma}: width {} ratio {ratio}", m.edge.width_px);
            }
            assert!(m.edge.rays >= 48, "sigma {sigma}: only {} rays", m.edge.rays);
        }
    }

    #[test]
    fn motion_blur_shows_as_anisotropy() {
        let base = gauss(&eye_image(W, H, (150.0, 150.0), 35.0), W, H, 1.0);
        let iso = measure_eye(&luma(&base, W, H), ALL, 100.0).unwrap();
        assert!(iso.edge.anisotropy < 1.15, "isotropic anisotropy {}", iso.edge.anisotropy);
        let mv = measure_eye(&luma(&motion(&base, W, H, 8), W, H), ALL, 100.0).unwrap();
        assert!(mv.edge.anisotropy > 1.3, "motion anisotropy {}", mv.edge.anisotropy);
        assert!(mv.edge.width_px > iso.edge.width_px);
    }

    /// Fine texture passes the peaking test while sharp and fails it once
    /// blurred. (A single hard edge is different: moderate blur widens the
    /// band above threshold, which is why coverage is a texture measure and
    /// the edge width, not coverage, is the primary sharpness score.)
    #[test]
    fn coverage_drops_with_blur() {
        let texture = gauss(&noise(&vec![128.0; W * H], 60.0, 7), W, H, 1.0);
        let sharp = coverage(&luma(&texture, W, H), ALL);
        let soft = coverage(&luma(&gauss(&texture, W, H, 3.0), W, H), ALL);
        assert!(sharp > 0.05 && sharp > 3.0 * soft, "sharp {sharp} soft {soft}");
    }

    #[test]
    fn noise_barely_moves_the_width() {
        let base = gauss(&eye_image(W, H, (150.0, 150.0), 35.0), W, H, 1.0);
        let clean = measure_eye(&luma(&base, W, H), ALL, 100.0).unwrap().edge.width_px;
        let noisy = measure_eye(&luma(&noise(&base, 4.0, 3), W, H), ALL, 100.0).unwrap().edge.width_px;
        assert!((noisy - clean).abs() < 0.15 * clean, "clean {clean} noisy {noisy}");
    }
}
