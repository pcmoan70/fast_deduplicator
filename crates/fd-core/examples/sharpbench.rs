//! Sharpness-metric benchmark with synthetic ground truth.
//!
//! Loads a real frame at native resolution, degrades a window around a point
//! (Gaussian blur, noise, sub-pixel shift, exposure) at NATIVE scale, then
//! evaluates candidate metrics at the working resolutions the app uses
//! (2048 / 1638 px long edge) and on a native-res crop. Prints, per metric,
//! how strongly it separates sharp from blurred, how much noise inflates it,
//! and how stable it is under jitter.
//!
//!   cargo run --release -p fd-core --example sharpbench -- \
//!       [--point 0.5,0.4] [--half 256] [--window 2048] [--burst] <files...>
//!
//! `--burst`: score production-identical working-res lumas of the given files
//! with every metric (global max-over-tiles + ROI at the point) and print the
//! rank order and spread per metric.

use std::time::Instant;

use fd_core::decode::{self, Luma};
use fd_core::eye;
use fd_core::formats::{self, Source};
use fd_core::score::{self, Rect};

const DEFAULT_ROI_FRAC: f32 = 96.0 / 1620.0;

// ---------- f32 image + degradations ----------

#[derive(Clone)]
struct Img {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl Img {
    fn from_luma(l: &Luma) -> Img {
        Img { w: l.width, h: l.height, px: l.pixels.iter().map(|&v| v as f32).collect() }
    }
    fn to_luma(&self) -> Luma {
        Luma {
            width: self.w,
            height: self.h,
            pixels: self.px.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect(),
        }
    }
    fn crop(&self, x0: usize, y0: usize, w: usize, h: usize) -> Img {
        let mut px = Vec::with_capacity(w * h);
        for y in y0..y0 + h {
            px.extend_from_slice(&self.px[y * self.w + x0..y * self.w + x0 + w]);
        }
        Img { w, h, px }
    }
    fn gauss(&self, sigma: f32) -> Img {
        if sigma <= 0.0 {
            return self.clone();
        }
        let r = (3.0 * sigma).ceil() as isize;
        let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let norm: f32 = k.iter().sum();
        let (w, h) = (self.w as isize, self.h as isize);
        let mut tmp = vec![0f32; self.px.len()];
        for y in 0..h {
            for x in 0..w {
                let mut s = 0.0;
                for (j, kv) in k.iter().enumerate() {
                    let xx = (x + j as isize - r).clamp(0, w - 1);
                    s += kv * self.px[(y * w + xx) as usize];
                }
                tmp[(y * w + x) as usize] = s / norm;
            }
        }
        let mut out = vec![0f32; self.px.len()];
        for y in 0..h {
            for x in 0..w {
                let mut s = 0.0;
                for (j, kv) in k.iter().enumerate() {
                    let yy = (y + j as isize - r).clamp(0, h - 1);
                    s += kv * tmp[(yy * w + x) as usize];
                }
                out[(y * w + x) as usize] = s / norm;
            }
        }
        Img { w: self.w, h: self.h, px: out }
    }
    fn noise(&self, sigma: f32, seed: u32) -> Img {
        let mut state = seed;
        let mut uni = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1u32 << 24) as f32
        };
        let px = self
            .px
            .iter()
            .map(|&v| {
                let g: f32 = (0..12).map(|_| uni()).sum::<f32>() - 6.0;
                (v + sigma * g).clamp(0.0, 255.0)
            })
            .collect();
        Img { w: self.w, h: self.h, px }
    }
    fn shift(&self, dx: f32, dy: f32) -> Img {
        let (w, h) = (self.w, self.h);
        let at = |x: isize, y: isize| self.px[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x as f32 - dx, y as f32 - dy);
                let (x0, y0) = (sx.floor(), sy.floor());
                let (fx, fy) = (sx - x0, sy - y0);
                let (x0, y0) = (x0 as isize, y0 as isize);
                let v = at(x0, y0) * (1.0 - fx) * (1.0 - fy)
                    + at(x0 + 1, y0) * fx * (1.0 - fy)
                    + at(x0, y0 + 1) * (1.0 - fx) * fy
                    + at(x0 + 1, y0 + 1) * fx * fy;
                px.push(v);
            }
        }
        Img { w, h, px }
    }
    fn expose(&self, k: f32) -> Img {
        Img { w: self.w, h: self.h, px: self.px.iter().map(|v| (v * k).clamp(0.0, 255.0)).collect() }
    }
    fn downsample(&self, f: usize) -> Img {
        let (w, h) = (self.w / f, self.h / f);
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let mut s = 0.0;
                for yy in 0..f {
                    for xx in 0..f {
                        s += self.px[(y * f + yy) * self.w + x * f + xx];
                    }
                }
                px.push(s / (f * f) as f32);
            }
        }
        Img { w, h, px }
    }
}

// ---------- candidate metrics ----------

type Metric = fn(&Luma, Rect) -> f32;

/// The rect (plus `margin`) as f32 with the rect's offset inside the patch.
fn patch(l: &Luma, r: Rect, margin: usize) -> (Img, usize, usize) {
    let x0 = r.x.saturating_sub(margin);
    let y0 = r.y.saturating_sub(margin);
    let x1 = (r.x + r.w + margin).min(l.width);
    let y1 = (r.y + r.h + margin).min(l.height);
    let img = Img::from_luma(l).crop(x0, y0, x1 - x0, y1 - y0);
    (img, r.x - x0, r.y - y0)
}

fn stats(p: &Img, ox: usize, oy: usize, w: usize, h: usize) -> (f64, f64, f64) {
    let (mut s, mut s2, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for y in oy..oy + h {
        for x in ox..ox + w {
            let v = p.px[y * p.w + x] as f64;
            s += v;
            s2 += v * v;
            n += 1.0;
        }
    }
    let mean = s / n;
    (mean, (s2 / n - mean * mean).max(1.0), n)
}

/// Sobel gradient energy per interior pixel of the rect.
fn sobel_sq(p: &Img, ox: usize, oy: usize, w: usize, h: usize) -> Vec<f32> {
    let s = p.w;
    let mut out = Vec::with_capacity(w * h);
    for y in (oy + 1)..(oy + h - 1) {
        for x in (ox + 1)..(ox + w - 1) {
            let i = y * s + x;
            let (a, b, c) = (p.px[i - s - 1], p.px[i - s], p.px[i - s + 1]);
            let (d, f) = (p.px[i - 1], p.px[i + 1]);
            let (g, hh, k) = (p.px[i + s - 1], p.px[i + s], p.px[i + s + 1]);
            let gx = (c + 2.0 * f + k) - (a + 2.0 * d + g);
            let gy = (g + 2.0 * hh + k) - (a + 2.0 * b + c);
            out.push(gx * gx + gy * gy);
        }
    }
    out
}

fn mean(v: &[f32]) -> f64 {
    v.iter().map(|&x| x as f64).sum::<f64>() / v.len().max(1) as f64
}

/// (a) baseline: the app's current metric.
fn m_teng(l: &Luma, r: Rect) -> f32 {
    score::score_region(l, r).score
}

/// (b) variance of the 5-point Laplacian / luma variance.
fn m_lapv(l: &Luma, r: Rect) -> f32 {
    let (p, ox, oy) = patch(l, r, 1);
    let (_, var, _) = stats(&p, ox, oy, r.w, r.h);
    let s = p.w;
    let mut acc = 0.0f64;
    let mut n = 0.0f64;
    for y in (oy + 1)..(oy + r.h - 1) {
        for x in (ox + 1)..(ox + r.w - 1) {
            let i = y * s + x;
            let lap = p.px[i - 1] + p.px[i + 1] + p.px[i - s] + p.px[i + s] - 4.0 * p.px[i];
            acc += (lap * lap) as f64;
            n += 1.0;
        }
    }
    (acc / n / var) as f32
}

/// (c) Nayar sum-modified Laplacian / mean absolute deviation.
fn m_sml(l: &Luma, r: Rect) -> f32 {
    let (p, ox, oy) = patch(l, r, 1);
    let (m, _, _) = stats(&p, ox, oy, r.w, r.h);
    let s = p.w;
    let (mut acc, mut mad, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for y in (oy + 1)..(oy + r.h - 1) {
        for x in (ox + 1)..(ox + r.w - 1) {
            let i = y * s + x;
            let v = p.px[i];
            acc += ((p.px[i - 1] + p.px[i + 1] - 2.0 * v).abs() + (p.px[i - s] + p.px[i + s] - 2.0 * v).abs()) as f64;
            mad += (v as f64 - m).abs();
            n += 1.0;
        }
    }
    (acc / (mad.max(n)) ) as f32
}

/// (e) two-scale ratio: fine gradient energy over the same energy after a
/// sigma-2 Gaussian (no decimation), gated so flat patches score low.
fn m_ratio2(l: &Luma, r: Rect) -> f32 {
    let (p, ox, oy) = patch(l, r, 8);
    let coarse = p.gauss(2.0);
    let fine = mean(&sobel_sq(&p, ox, oy, r.w, r.h));
    let base = mean(&sobel_sq(&coarse, ox, oy, r.w, r.h)).max(1.0);
    (fine / base) as f32
}

/// (f) mean of the top 15% Sobel magnitudes / luma std-dev.
fn m_pct(l: &Luma, r: Rect) -> f32 {
    let (p, ox, oy) = patch(l, r, 1);
    let (_, var, _) = stats(&p, ox, oy, r.w, r.h);
    let mut mags: Vec<f32> = sobel_sq(&p, ox, oy, r.w, r.h).iter().map(|v| v.sqrt()).collect();
    if mags.is_empty() {
        return 0.0;
    }
    let k = (mags.len() * 15 / 100).max(1);
    let split = mags.len() - k;
    mags.select_nth_unstable_by(split, |a, b| a.partial_cmp(b).unwrap());
    (mean(&mags[split..]) / var.sqrt()) as f32
}

/// (g) Tenengrad / var on a sigma-0.7 pre-smoothed patch.
fn m_presm(l: &Luma, r: Rect) -> f32 {
    let (p, ox, oy) = patch(l, r, 3);
    let sm = p.gauss(0.7);
    let (_, var, _) = stats(&sm, ox, oy, r.w, r.h);
    (mean(&sobel_sq(&sm, ox, oy, r.w, r.h)) / var) as f32
}

/// (b') lapv/var with a contrast gate: x min(1, var/400), so a flat noisy
/// tile cannot win max-over-tiles (std >= 20 passes untouched).
fn m_lapv_gate(l: &Luma, r: Rect) -> f32 {
    let (p, ox, oy) = patch(l, r, 1);
    let (_, var, _) = stats(&p, ox, oy, r.w, r.h);
    m_lapv(l, r) * (var / 400.0).min(1.0) as f32
}

/// (b'') Laplacian energy / Sobel energy: amplitude-free slope of the
/// high band; white noise gives a constant, so flat tiles do not explode.
fn m_lap_grad(l: &Luma, r: Rect) -> f32 {
    let (p, ox, oy) = patch(l, r, 1);
    let s = p.w;
    let mut acc = 0.0f64;
    for y in (oy + 1)..(oy + r.h - 1) {
        for x in (ox + 1)..(ox + r.w - 1) {
            let i = y * s + x;
            let lap = p.px[i - 1] + p.px[i + 1] + p.px[i - s] + p.px[i + s] - 4.0 * p.px[i];
            acc += (lap * lap) as f64;
        }
    }
    let g = sobel_sq(&p, ox, oy, r.w, r.h);
    (acc / g.len().max(1) as f64 / mean(&g).max(1.0)) as f32
}

/// Laplacian variance after a Gaussian pre-smooth (LoG): tames native-res
/// noise while keeping the second-derivative sensitivity.
fn log_var(l: &Luma, r: Rect, sigma: f32) -> f32 {
    let (p, ox, oy) = patch(l, r, 4);
    let sm = p.gauss(sigma);
    let (_, var, _) = stats(&sm, ox, oy, r.w, r.h);
    let s = sm.w;
    let (mut acc, mut n) = (0.0f64, 0.0f64);
    for y in (oy + 1)..(oy + r.h - 1) {
        for x in (ox + 1)..(ox + r.w - 1) {
            let i = y * s + x;
            let lap = sm.px[i - 1] + sm.px[i + 1] + sm.px[i - s] + sm.px[i + s] - 4.0 * sm.px[i];
            acc += (lap * lap) as f64;
            n += 1.0;
        }
    }
    (acc / n / var) as f32
}
fn m_log1(l: &Luma, r: Rect) -> f32 {
    log_var(l, r, 1.0)
}
fn m_log2(l: &Luma, r: Rect) -> f32 {
    log_var(l, r, 2.0)
}

const METRICS: &[(&str, Metric)] = &[
    ("teng/var", m_teng),
    ("lapv/var", m_lapv),
    ("lapv*gate", m_lapv_gate),
    ("lap/grad", m_lap_grad),
    ("log1.0", m_log1),
    ("log2.0", m_log2),
    ("sml/mad", m_sml),
    ("ratio2", m_ratio2),
    ("pct15", m_pct),
    ("presm0.7", m_presm),
];

/// score_global's tiling (9x6, center weight, max) with an arbitrary metric.
fn global(m: Metric, l: &Luma) -> f32 {
    let (tx, ty) = (9usize, 6usize);
    let (tw, th) = (l.width / tx, l.height / ty);
    let mut best = 0.0f32;
    for j in 0..ty {
        for i in 0..tx {
            let s = m(l, Rect { x: i * tw, y: j * th, w: tw, h: th });
            let dx = (i as f32 + 0.5) / tx as f32 - 0.5;
            let dy = (j as f32 + 0.5) / ty as f32 - 0.5;
            best = best.max(s * (1.0 - 0.8 * (dx * dx + dy * dy).sqrt()));
        }
    }
    best
}

fn roi_rect(l: &Luma, px: f32, py: f32, frac: f32) -> Rect {
    let half = (frac * l.width.max(l.height) as f32).round() as usize;
    let cx = ((px * l.width as f32) as usize).clamp(half, l.width - half);
    let cy = ((py * l.height as f32) as usize).clamp(half, l.height - half);
    Rect { x: cx - half, y: cy - half, w: 2 * half, h: 2 * half }
}

// ---------- benchmark ----------

fn load_native(path: &str) -> (Luma, fd_core::meta::FileMeta) {
    let (meta, _) = formats::parse_header(std::path::Path::new(path)).expect("parse");
    let (jpeg, _, _) = formats::extract_jpeg(&meta, Source::Full).expect("extract").expect("no full");
    (decode::decode_luma_scaled(&jpeg, usize::MAX / 2).expect("decode"), meta)
}

struct Target {
    name: &'static str,
    /// Downsample factor (1 = native crop).
    factor: usize,
    /// Long edge of the full frame at this target, for the ROI size.
    long: usize,
}

fn bench_file(path: &str, point: (f32, f32), half: usize, window: usize) {
    let (native, meta) = load_native(path);
    let full = Img::from_luma(&native);
    let cx = ((point.0 * full.w as f32) as usize).clamp(window / 2, full.w - window / 2);
    let cy = ((point.1 * full.h as f32) as usize).clamp(window / 2, full.h - window / 2);
    let win = full.crop(cx - window / 2, cy - window / 2, window, window);
    println!(
        "\n#### {} {}x{} orientation {} · window {}px @ ({cx},{cy})",
        path.rsplit('/').next().unwrap(), meta.width, meta.height, meta.orientation, window
    );

    // Degradations at native scale (noise fields share a seed so they are
    // common-mode across variants, like sensor noise within a burst).
    let mut variants: Vec<(String, Img)> = vec![("sharp".into(), win.clone())];
    for s in [0.5f32, 1.0, 1.5, 2.0, 3.0, 4.0] {
        variants.push((format!("blur{s}"), win.gauss(s)));
    }
    for n in [2.0f32, 4.0, 8.0] {
        variants.push((format!("noise{n}"), win.noise(n, 7)));
    }
    variants.push(("blur1+n8".into(), win.gauss(1.0).noise(8.0, 7)));
    variants.push(("shift0.3".into(), win.shift(0.3, 0.3)));
    variants.push(("shift0.7".into(), win.shift(0.7, 0.7)));
    variants.push(("exp0.8".into(), win.expose(0.8)));
    variants.push(("exp1.2".into(), win.expose(1.2)));
    let flat = Img { w: win.w, h: win.h, px: vec![128.0; win.w * win.h] }.noise(4.0, 11);
    variants.push(("flatnoise".into(), flat));

    let targets = [
        Target { name: "w4096", factor: 2, long: full.w.max(full.h) / 2 },
        Target { name: "w2048", factor: 4, long: full.w.max(full.h) / 4 },
        Target { name: "w1638", factor: 5, long: full.w.max(full.h) / 5 },
        Target { name: "native512", factor: 1, long: full.w.max(full.h) },
    ];
    for t in &targets {
        // Per variant: the target luma and the ROI rect within it.
        let lumas: Vec<(&str, Luma, Rect)> = variants
            .iter()
            .map(|(name, img)| {
                let (luma, rect) = if t.factor == 1 {
                    let c = img.crop(img.w / 2 - half - 2, img.h / 2 - half - 2, 2 * half + 4, 2 * half + 4);
                    (c.to_luma(), Rect { x: 2, y: 2, w: 2 * half, h: 2 * half })
                } else {
                    let d = img.downsample(t.factor).to_luma();
                    let hr = (DEFAULT_ROI_FRAC * t.long as f32).round() as usize;
                    let r = Rect { x: d.width / 2 - hr, y: d.height / 2 - hr, w: 2 * hr, h: 2 * hr };
                    (d, r)
                };
                (name.as_str(), luma, rect)
            })
            .collect();
        let (_, l0, r0) = &lumas[0];
        println!(
            "== target {} ({}x{}, roi {}px)  columns: D = sharp/blurred, N = noise inflation, Dn1 = sharp+n8 / blur1+n8, sh/rj = shift / roi±2 jitter %, exp %, flat = flatnoise/sharp",
            t.name, l0.width, l0.height, r0.w
        );
        println!(
            "{:<10} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} | {:>5} {:>5} {:>5} {:>5} | {:>5} {:>5} {:>5} | {:>5} | {:>7}",
            "metric", "D0.5", "D1", "D1.5", "D2", "D3", "D4", "N2", "N4", "N8", "Dn1", "sh%", "rj%", "exp%", "flat", "us"
        );
        for (name, m) in METRICS {
            let s = |v: &str| -> f32 {
                let (_, l, r) = lumas.iter().find(|(n, _, _)| *n == v).unwrap();
                m(l, *r)
            };
            let t0 = Instant::now();
            let sharp = s("sharp");
            let us = t0.elapsed().as_micros();
            let d = |v: &str| sharp / s(v).max(1e-9);
            let rel = |v: &str| (s(v) / sharp - 1.0).abs() * 100.0;
            // ROI placement jitter: move the rect by ±2 px on the sharp target.
            let rj = {
                let mut worst = 0.0f32;
                for (dx, dy) in [(2i64, 2i64), (-2, 2), (2, -2), (-2, -2)] {
                    let r = Rect {
                        x: (r0.x as i64 + dx).clamp(0, (l0.width - r0.w) as i64) as usize,
                        y: (r0.y as i64 + dy).clamp(0, (l0.height - r0.h) as i64) as usize,
                        w: r0.w,
                        h: r0.h,
                    };
                    worst = worst.max((m(l0, r) / sharp - 1.0).abs() * 100.0);
                }
                worst
            };
            println!(
                "{:<10} {:>5.2} {:>5.2} {:>5.2} {:>5.2} {:>5.2} {:>5.2} | {:>5.2} {:>5.2} {:>5.2} {:>5.2} | {:>5.1} {:>5.1} {:>5.1} | {:>5.2} | {:>7}",
                name,
                d("blur0.5"), d("blur1"), d("blur1.5"), d("blur2"), d("blur3"), d("blur4"),
                s("noise2") / sharp, s("noise4") / sharp, s("noise8") / sharp,
                s("noise8") / s("blur1+n8").max(1e-9),
                rel("shift0.3").max(rel("shift0.7")), rj,
                rel("exp0.8").max(rel("exp1.2")),
                s("flatnoise") / sharp,
                us
            );
        }
    }
}

fn burst(files: &[String], point: (f32, f32), seed: Option<&str>) {
    println!("\n#### burst mode: production working-res luma (embedded source), global + ROI @ ({:.3},{:.3}) seed {:?}", point.0, point.1, seed);
    let lumas: Vec<(String, Luma)> = files
        .iter()
        .map(|f| {
            let (meta, _) = formats::parse_header(std::path::Path::new(f)).expect("parse");
            let (jpeg, _, _) = formats::extract_jpeg(&meta, Source::Embedded).expect("extract").expect("no preview");
            let l = decode::decode_luma_scaled(&jpeg, 1600).expect("decode").upright(meta.orientation);
            (f.rsplit('/').next().unwrap().to_string(), l)
        })
        .collect();
    // Per-file ROI point: the app's tracker from the seed file, else fixed.
    let mut pts: Vec<(f32, f32, f32)> = vec![(point.0, point.1, 1.0); lumas.len()];
    if let Some(si) = seed.and_then(|s| lumas.iter().position(|(n, _)| n.contains(s))) {
        let l0 = &lumas[si].1;
        let (sx, sy) = (point.0 * l0.width as f32, point.1 * l0.height as f32);
        for dir in [1i64, -1] {
            let mut tr = fd_core::track::Tracker::new(l0, sx, sy).expect("seed template");
            let mut i = si as i64 + dir;
            while i >= 0 && (i as usize) < lumas.len() {
                let l = &lumas[i as usize].1;
                let (x, y, c) = tr.step(l);
                pts[i as usize] = (x / l.width as f32, y / l.height as f32, c);
                i += dir;
            }
        }
    }
    let mut rows: Vec<(String, Vec<f32>, Vec<f32>)> = Vec::new();
    for (k, (name, l)) in lumas.iter().enumerate() {
        let (px, py, conf) = pts[k];
        let r = roi_rect(l, px, py, DEFAULT_ROI_FRAC);
        println!("{name}: point ({px:.3},{py:.3}) conf {conf:.2}");
        let g: Vec<f32> = METRICS.iter().map(|(_, m)| global(*m, l)).collect();
        let roi: Vec<f32> = METRICS.iter().map(|(_, m)| m(l, r)).collect();
        rows.push((name.clone(), g, roi));
    }
    print!("{:<14}", "file");
    for (n, _) in METRICS {
        print!(" {:>8}g {:>8}r", n, n);
    }
    println!();
    for (name, g, roi) in &rows {
        print!("{:<14}", name);
        for i in 0..METRICS.len() {
            print!(" {:>9.2} {:>9.2}", g[i], roi[i]);
        }
        println!();
    }
    println!("\nROI ranking (best first) and spread max/min per metric:");
    for (i, (n, _)) in METRICS.iter().enumerate() {
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| rows[b].2[i].partial_cmp(&rows[a].2[i]).unwrap());
        let max = rows[order[0]].2[i];
        let min = rows[*order.last().unwrap()].2[i];
        let names: Vec<&str> = order.iter().map(|&k| rows[k].0.trim_end_matches(".JPG")).collect();
        println!("{:<10} spread {:>5.2}x  {}", n, max / min.max(1e-9), names.join(" > "));
    }
}

/// Eye mode: for each file, crop the native eye region (camera eye box, else
/// `--point`), segment the pupil, measure the rim width, and write a 256px
/// crop with the fitted ellipse ring so segmentation and ranking can be
/// checked by eye. Files are named by rank; `montage.png` tiles them.
fn eyes(files: &[String], point: (f32, f32), out: Option<&str>) {
    struct Row { name: String, w75: f32, w50: f32, w90: f32, anis: f32, rays: usize, r: f32, lap: f32, cov: f32, ms: u128, tile: Vec<u8>, src: &'static str }
    let mut rows: Vec<Row> = Vec::new();
    for f in files {
        let name = f.rsplit('/').next().unwrap().to_string();
        let (meta, _) = formats::parse_header(std::path::Path::new(f)).expect("parse");
        let (jpeg, _, _) = formats::extract_jpeg(&meta, Source::Full).expect("extract").expect("no full");
        let inv = decode::inverse_orientation(meta.orientation);
        let (dw, dh) = meta.display_dims();
        // Eye centre in stored coordinates and the search half-size in px.
        let (ux, uy, half, src) = match meta.af_box_eye() {
            Some(b) => (b.cx, b.cy, (0.75 * (b.w * dw as f32).max(b.h * dh as f32)).max(100.0), "camera"),
            None => (point.0, point.1, 0.025 * meta.width.max(meta.height) as f32, "point"),
        };
        let (sx, sy) = decode::orient_norm(ux, uy, inv);
        let (cx, cy) = ((sx * meta.width as f32) as usize, (sy * meta.height as f32) as usize);
        let half = half as usize;
        let t = Instant::now();
        let (crop, x0, y0) = decode::decode_luma_crop(&jpeg, cx.saturating_sub(half), cy.saturating_sub(half), 2 * half, 2 * half).expect("crop");
        // A nominal fallback circle (area 0) is not a pupil: report no measurement.
        let m = eye::measure_eye(&crop, Rect { x: 0, y: 0, w: crop.width, h: crop.height }, 0.5 * half as f32).filter(|m| m.blob.area > 0);
        let ms = t.elapsed().as_millis();
        // Production Laplacian ROI at the same (upright) point for comparison.
        let (pj, _, _) = formats::extract_jpeg(&meta, Source::Embedded).expect("extract").expect("no preview");
        let l = decode::decode_luma_scaled(&pj, 1600).expect("decode").upright(meta.orientation);
        let lap = score::score_region(&l, roi_rect(&l, ux, uy, DEFAULT_ROI_FRAC)).score;
        // Coverage at working resolution, as the app counts it.
        let cov = eye::coverage(&l, roi_rect(&l, ux, uy, 0.5 * DEFAULT_ROI_FRAC));
        let (bx, by) = m.map(|m| (m.blob.cx, m.blob.cy)).unwrap_or((crop.width as f32 / 2.0, crop.height as f32 / 2.0));
        // 256x256 RGB tile centred on the blob, ellipse ring in red.
        let mut tile = vec![0u8; 256 * 256 * 3];
        let (ox, oy) = ((bx as i64 - 128).clamp(0, crop.width as i64 - 256), (by as i64 - 128).clamp(0, crop.height as i64 - 256));
        for y in 0..256usize {
            for x in 0..256usize {
                let (sx, sy) = (ox as usize + x, oy as usize + y);
                let v = if sx < crop.width && sy < crop.height { crop.pixels[sy * crop.width + sx] } else { 0 };
                tile[(y * 256 + x) * 3..(y * 256 + x) * 3 + 3].copy_from_slice(&[v, v, v]);
            }
        }
        if let Some(m) = &m {
            let b = m.blob;
            for k in 0..720 {
                let psi = std::f32::consts::TAU * k as f32 / 720.0;
                let (lx, ly) = (b.a * psi.cos(), b.b * psi.sin());
                let px = b.cx + b.phi.cos() * lx - b.phi.sin() * ly - ox as f32;
                let py = b.cy + b.phi.sin() * lx + b.phi.cos() * ly - oy as f32;
                if px >= 0.0 && py >= 0.0 && (px as usize) < 256 && (py as usize) < 256 {
                    let i = (py as usize * 256 + px as usize) * 3;
                    tile[i..i + 3].copy_from_slice(&[255, 40, 40]);
                }
            }
        }
        println!(
            "{name}: crop {}x{} @({x0},{y0}) {src} half {half} orient {} · {} · lap {lap:.1} · {ms} ms",
            crop.width, crop.height, meta.orientation,
            match &m {
                Some(m) => format!("width p75 {:.2} p50 {:.2} p90 {:.2} px · anis {:.2} · rays {} · r {:.0}px", m.edge.width_px, m.edge.median_px, m.edge.width_px * 0.0 + m.edge.median_px * m.edge.anisotropy, m.edge.anisotropy, m.edge.rays, (m.blob.a * m.blob.b).sqrt()),
                None => "no eye measured".to_string(),
            }
        );
        let (w75, w50, w90, anis, rays, r) = match &m {
            Some(m) => (m.edge.width_px, m.edge.median_px, m.edge.median_px * m.edge.anisotropy, m.edge.anisotropy, m.edge.rays, (m.blob.a * m.blob.b).sqrt()),
            None => (f32::INFINITY, 0.0, 0.0, 0.0, 0, 0.0),
        };
        rows.push(Row { name, w75, w50, w90, anis, rays, r, lap, cov, ms, tile, src });
    }
    rows.sort_by(|a, b| a.w75.partial_cmp(&b.w75).unwrap());
    println!("\nranking by eye width (sharpest first):");
    for (i, r) in rows.iter().enumerate() {
        println!("{:>2}. {:<14} {:>5.2} px  (p50 {:.2} p90 {:.2} anis {:.2} rays {} r {:.0}) cov {:>4.1}% lap {:.1} {} {} ms", i + 1, r.name, r.w75, r.w50, r.w90, r.anis, r.rays, r.r, 100.0 * r.cov, r.lap, r.src, r.ms);
    }
    let mut by_cov: Vec<&Row> = rows.iter().collect();
    by_cov.sort_by(|a, b| b.cov.partial_cmp(&a.cov).unwrap());
    println!("\nranking by focus coverage (most first): {}", by_cov.iter().map(|r| format!("{} {:.1}%", r.name.trim_end_matches(".JPG"), 100.0 * r.cov)).collect::<Vec<_>>().join(" > "));
    if let Some(dir) = out {
        std::fs::create_dir_all(dir).unwrap();
        let cols = 8usize.min(rows.len().max(1));
        let nrows = rows.len().div_ceil(cols);
        let mut montage = image::RgbImage::new((256 * cols) as u32, (256 * nrows) as u32);
        for (i, r) in rows.iter().enumerate() {
            let img = image::RgbImage::from_raw(256, 256, r.tile.clone()).unwrap();
            let stem = r.name.trim_end_matches(".JPG").trim_end_matches(".CR3");
            img.save(format!("{dir}/eye_{:02}_{:.2}px_{stem}.png", i + 1, r.w75)).unwrap();
            let (tx, ty) = (((i % cols) * 256) as u32, ((i / cols) * 256) as u32);
            image::imageops::replace(&mut montage, &img, tx as i64, ty as i64);
        }
        montage.save(format!("{dir}/montage.png")).unwrap();
        println!("wrote {} tiles + montage to {dir}", rows.len());
    }
}

fn main() {
    let mut point = (0.5f32, 0.5f32);
    let mut half = 256usize;
    let mut window = 2048usize;
    let mut burst_mode = false;
    let mut seed: Option<String> = None;
    let mut eyes_mode = false;
    let mut out: Option<String> = None;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--point" => {
                let v = args.next().unwrap();
                let (x, y) = v.split_once(',').unwrap();
                point = (x.parse().unwrap(), y.parse().unwrap());
            }
            "--half" => half = args.next().unwrap().parse().unwrap(),
            "--window" => window = args.next().unwrap().parse().unwrap(),
            "--burst" => burst_mode = true,
            "--seed" => seed = args.next(),
            "--eyes" => eyes_mode = true,
            "--out" => out = args.next(),
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        eprintln!("usage: sharpbench [--point x,y] [--half N] [--window N] [--burst [--seed NAME]] [--eyes [--out DIR]] <files...>");
        std::process::exit(2);
    }
    if eyes_mode {
        eyes(&files, point, out.as_deref());
    } else if burst_mode {
        burst(&files, point, seed.as_deref());
    } else {
        for f in &files {
            bench_file(f, point, half, window);
        }
    }
}
