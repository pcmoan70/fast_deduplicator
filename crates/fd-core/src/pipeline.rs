//! Background engine for the GUI: a scan thread plus a worker pool draining
//! a priority job queue. The UI requests thumbs/previews/scores for what is
//! visible; higher-priority jobs (the frame under the cursor) jump the
//! queue. No egui dependency — the CLI and future wasm build reuse this.

use std::collections::BinaryHeap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::cache::{file_key, Cache};
use crate::decode;
use crate::eye;
use crate::formats::{self, Source};
use crate::meta::FileMeta;
use crate::score;
use crate::track::{Tracker, CONF_OK};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobKind {
    /// Small grid thumbnail (~200 px), always from the embedded preview.
    Thumb,
    /// Main-view image (~1600-2000 px) from the engine's `Source`.
    Preview,
    /// Full-resolution embedded JPEG for 100% zoom.
    Full,
    /// Global sharpness score (cache-backed).
    Score,
}

pub enum Event {
    /// Scan finished; metas are indexed by position (the `idx` in the other
    /// events). Errors were skipped and counted.
    Scanned {
        metas: Vec<FileMeta>,
        errors: usize,
    },
    Image {
        kind: JobKind,
        idx: usize,
        rgba: Vec<u8>,
        width: usize,
        height: usize,
    },
    Score {
        idx: usize,
        score: f32,
    },
    /// One tracked frame of an ROI track (streams as the wavefront runs).
    TrackPoint {
        req_id: u64,
        idx: usize,
        /// Position in normalized image coordinates (0..1).
        x: f32,
        y: f32,
        /// NCC confidence (1.0 for the seed frame).
        conf: f32,
        /// Sharpness at the tracked point.
        roi_score: f32,
        /// Fraction of the focus area passing the peaking test at working
        /// resolution (what the preview overlay paints).
        coverage: f32,
    },
    /// Native-resolution pupil measurement for one frame, after its
    /// `TrackPoint`; `x, y` is the pupil centre (normalized, upright).
    EyePoint {
        req_id: u64,
        idx: usize,
        x: f32,
        y: f32,
        kind: RoiKind,
        eye: EyeMark,
    },
    /// Every frame of the track has been measured (or given up on).
    TrackDone { req_id: u64 },
}

/// A click-and-track request over one burst (frames in capture order, as
/// meta indices). `seeds` are the user's pins, (meta idx, x, y) with
/// normalized 0..1 coordinates; every frame follows its nearest pin.
pub struct TrackRequest {
    pub req_id: u64,
    pub frames: Vec<usize>,
    pub seeds: Vec<(usize, f32, f32)>,
    /// Eye search half-size for frames WITHOUT a camera eye box, as a
    /// fraction of the image's long edge.
    pub roi_frac: f32,
    /// Use the camera's eye-AF frames as implicit pins.
    pub use_af: bool,
}

/// Where a frame's focus point and eye measurement came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoiKind {
    /// The camera's eye-AF frame, pupil measured at native resolution.
    EyeCamera,
    /// A pinned or tracked point, pupil measured at native resolution.
    EyeTracked,
}

/// Native-resolution pupil measurement (see `eye`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeMark {
    /// Pupil radius as a fraction of the long edge (for drawing).
    pub r_frac: f32,
    /// 10-90% rise width of the pupil rim in native px (lower = sharper).
    pub width_px: f32,
    /// p90/p50 of the rim widths; above ~1.3 the blur is directional.
    pub anisotropy: f32,
}

/// Working-resolution patch for the interim/fallback ROI score (half-size as
/// a fraction of the long edge: 96 px on a 1620 px preview).
const AREA_FRAC: f32 = 96.0 / 1620.0;

#[derive(Eq, PartialEq)]
struct Job {
    priority: i32,
    seq: u64, // tie-break: newer requests first (locality of navigation)
    kind: JobKind,
    idx: usize,
}

impl Ord for Job {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority
            .cmp(&other.priority)
            .then(self.seq.cmp(&other.seq))
    }
}

impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct QueueState {
    heap: BinaryHeap<Job>,
    queued: HashSet<(JobKind, usize)>,
    tracks: Vec<TrackRequest>,
    metas: Option<Arc<Vec<FileMeta>>>,
    seq: u64,
    shutdown: bool,
}

struct Shared {
    q: Mutex<QueueState>,
    cv: Condvar,
    events: crossbeam_channel::Sender<Event>,
    cache: Mutex<Option<Cache>>,
    /// Newest track request id; a running track aborts when superseded.
    latest_track: AtomicU64,
    /// Set by workers on repaint-worthy events; cleared by the UI.
    dirty: AtomicBool,
    /// What previews, scores and tracks are computed from.
    source: Source,
    /// Jobs and tracks executing right now (busy indicator).
    active: AtomicUsize,
    /// Display-only auto brighten of decoded RGBA (never the scored luma).
    brighten: AtomicBool,
    /// Display-only focus-peaking overlay on previews and full-res views.
    peaking: AtomicBool,
}

pub struct Engine {
    shared: Arc<Shared>,
    pub events: crossbeam_channel::Receiver<Event>,
}

impl Engine {
    /// Spawn the scan for `dir` plus `workers` decode threads. Score results
    /// are cached in `cache_path`, keyed by `source`.
    pub fn start(dir: PathBuf, cache_path: PathBuf, workers: usize, source: Source) -> Engine {
        let (tx, rx) = crossbeam_channel::unbounded();
        let shared = Arc::new(Shared {
            q: Mutex::new(QueueState {
                heap: BinaryHeap::new(),
                queued: HashSet::new(),
                tracks: Vec::new(),
                metas: None,
                seq: 0,
                shutdown: false,
            }),
            cv: Condvar::new(),
            events: tx,
            cache: Mutex::new(Cache::open(&cache_path).ok()),
            latest_track: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
            source,
            active: AtomicUsize::new(0),
            brighten: AtomicBool::new(false),
            peaking: AtomicBool::new(false),
        });

        // Scan thread: header-parse everything, publish metas.
        {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                let paths = collect_paths(&dir);
                let results: Vec<Option<FileMeta>> = {
                    // Simple manual parallelism to avoid a rayon dependency
                    // in the engine: chunk across scan threads.
                    let n = std::thread::available_parallelism()
                        .map(|v| v.get())
                        .unwrap_or(4);
                    let paths = Arc::new(paths);
                    let mut handles = Vec::new();
                    for t in 0..n {
                        let paths = Arc::clone(&paths);
                        handles.push(std::thread::spawn(move || {
                            let mut out = Vec::new();
                            let mut i = t;
                            while i < paths.len() {
                                out.push((
                                    i,
                                    formats::parse_header(&paths[i]).ok().map(|(m, _)| m),
                                ));
                                i += n;
                            }
                            out
                        }));
                    }
                    let mut all: Vec<Option<FileMeta>> = Vec::new();
                    all.resize_with(paths.len(), || None);
                    for h in handles {
                        for (i, m) in h.join().unwrap_or_default() {
                            all[i] = m;
                        }
                    }
                    all
                };
                let errors = results.iter().filter(|r| r.is_none()).count();
                let metas: Vec<FileMeta> = results.into_iter().flatten().collect();
                {
                    let mut q = shared.q.lock().unwrap();
                    q.metas = Some(Arc::new(metas.clone()));
                }
                shared.dirty.store(true, Ordering::Relaxed);
                let _ = shared.events.send(Event::Scanned { metas, errors });
            });
        }

        // Worker pool.
        for _ in 0..workers.max(1) {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || worker_loop(shared));
        }

        Engine { shared, events: rx }
    }

    /// Queue a job (idempotent while queued). Higher priority runs first.
    pub fn request(&self, kind: JobKind, idx: usize, priority: i32) {
        let mut q = self.shared.q.lock().unwrap();
        if q.metas.is_none() || !q.queued.insert((kind, idx)) {
            return;
        }
        q.seq += 1;
        let seq = q.seq;
        q.heap.push(Job {
            priority,
            seq,
            kind,
            idx,
        });
        drop(q);
        self.shared.cv.notify_one();
    }

    /// Start (or replace) a click-and-track run. Any in-flight track for an
    /// older request aborts at its next frame boundary.
    pub fn request_track(&self, req: TrackRequest) {
        self.shared.latest_track.store(req.req_id, Ordering::Relaxed);
        let mut q = self.shared.q.lock().unwrap();
        if q.metas.is_none() {
            return;
        }
        q.tracks.push(req);
        drop(q);
        self.shared.cv.notify_one();
    }

    /// True if a repaint-worthy event arrived since the last call.
    pub fn take_dirty(&self) -> bool {
        self.shared.dirty.swap(false, Ordering::Relaxed)
    }

    pub fn pending(&self) -> usize {
        self.shared.q.lock().unwrap().heap.len()
    }

    /// Queued plus executing work, so the UI can show it is busy.
    pub fn busy(&self) -> usize {
        let q = self.shared.q.lock().unwrap();
        q.heap.len() + q.tracks.len() + self.shared.active.load(Ordering::Relaxed)
    }

    /// Display-only: lift dark frames in the decoded previews from now on.
    pub fn set_brighten(&self, on: bool) {
        self.shared.brighten.store(on, Ordering::Relaxed);
    }

    /// Display-only: paint focus peaking on previews and full-res views.
    pub fn set_peaking(&self, on: bool) {
        self.shared.peaking.store(on, Ordering::Relaxed);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let mut q = self.shared.q.lock().unwrap();
        q.shutdown = true;
        drop(q);
        self.shared.cv.notify_all();
    }
}

pub fn collect_paths(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if formats::kind_from_ext(&p).is_some() {
                paths.push(p);
            }
        }
    }
    paths.sort();
    paths
}

fn worker_loop(shared: Arc<Shared>) {
    loop {
        let (job, metas) = {
            let mut q = shared.q.lock().unwrap();
            loop {
                if q.shutdown {
                    return;
                }
                if let Some(track) = q.tracks.pop() {
                    let metas = q.metas.clone().unwrap();
                    drop(q);
                    shared.active.fetch_add(1, Ordering::Relaxed);
                    run_track(&shared, &metas, track);
                    shared.active.fetch_sub(1, Ordering::Relaxed);
                    shared.dirty.store(true, Ordering::Relaxed);
                    q = shared.q.lock().unwrap();
                    continue;
                }
                if let Some(job) = q.heap.pop() {
                    q.queued.remove(&(job.kind, job.idx));
                    let metas = q.metas.clone().unwrap();
                    break (job, metas);
                }
                q = shared.cv.wait(q).unwrap();
            }
        };
        let Some(meta) = metas.get(job.idx) else {
            continue;
        };
        shared.active.fetch_add(1, Ordering::Relaxed);
        match job.kind {
            JobKind::Thumb | JobKind::Preview | JobKind::Full => {
                // Thumbs stay embedded (DCT scaling stops at 1/8, so a full-image
                // thumb would be ~1000 px); 100% zoom is always the full image.
                let (min, src) = match job.kind {
                    JobKind::Thumb => (200, Source::Embedded),
                    JobKind::Preview => (1600, shared.source),
                    _ => (usize::MAX / 2, Source::Full),
                };
                if let Ok(Some((jpeg, _, _))) = formats::extract_jpeg(meta, src) {
                    if let Ok(img) = decode::decode_rgba_scaled(&jpeg, min) {
                        let mut img = img.upright(meta.orientation);
                        if shared.brighten.load(Ordering::Relaxed) {
                            decode::auto_brighten(&mut img);
                        }
                        if shared.peaking.load(Ordering::Relaxed) && job.kind != JobKind::Thumb {
                            decode::peaking_overlay(&mut img);
                        }
                        let _ = shared.events.send(Event::Image {
                            kind: job.kind,
                            idx: job.idx,
                            rgba: img.pixels,
                            width: img.width,
                            height: img.height,
                        });
                    }
                }
            }
            JobKind::Score => {
                let key = file_key(meta, shared.source);
                let cached = {
                    let cache = shared.cache.lock().unwrap();
                    cache.as_ref().and_then(|c| c.get_score(&key))
                };
                let s = cached.or_else(|| {
                    let (jpeg, _, _) = formats::extract_jpeg(meta, shared.source).ok()??;
                    let luma = decode::decode_luma_scaled(&jpeg, 1600).ok()?;
                    let s = score::score_global(&luma).score;
                    let mut cache = shared.cache.lock().unwrap();
                    if let Some(c) = cache.as_mut() {
                        let _ = c.put_scores(std::iter::once((key.as_str(), s)));
                    }
                    Some(s)
                });
                if let Some(score) = s {
                    let _ = shared.events.send(Event::Score {
                        idx: job.idx,
                        score,
                    });
                }
            }
        }
        shared.active.fetch_sub(1, Ordering::Relaxed);
        shared.dirty.store(true, Ordering::Relaxed);
    }
}

/// Luma in display orientation, so seed clicks and emitted points share the
/// upright normalized space the GUI draws in.
fn track_luma(meta: &FileMeta, source: Source) -> Option<decode::Luma> {
    let (jpeg, _, _) = formats::extract_jpeg(meta, source).ok()??;
    Some(decode::decode_luma_scaled(&jpeg, 1600).ok()?.upright(meta.orientation))
}

/// Sharpness in a patch centered on the tracked point; `frac` is the patch
/// half-size as a fraction of the long edge.
fn roi_score(luma: &decode::Luma, x: f32, y: f32, frac: f32) -> f32 {
    let long = luma.width.max(luma.height) as f32;
    let r = ((frac * long) as usize).clamp(8, luma.width.min(luma.height) / 2);
    let cx = (x as usize).clamp(r, luma.width.saturating_sub(r));
    let cy = (y as usize).clamp(r, luma.height.saturating_sub(r));
    score::score_region(
        luma,
        score::Rect {
            x: cx - r,
            y: cy - r,
            w: 2 * r,
            h: 2 * r,
        },
    )
    .score
}

fn run_track(shared: &Shared, metas: &[FileMeta], req: TrackRequest) {
    let stale = || shared.latest_track.load(Ordering::Relaxed) != req.req_id;
    // Pins: the user's, plus the camera's eye frames as implicit pins on
    // frames without a user pin. Frames with neither follow their nearest pin
    // by NCC; ties go to the later pin, the one the user placed most recently.
    let mut seeds = req.seeds.clone();
    let mut camera: Vec<usize> = Vec::new();
    if req.use_af {
        for &f in &req.frames {
            if seeds.iter().all(|s| s.0 != f) {
                if let Some(b) = metas[f].af_box_eye() {
                    seeds.push((f, b.cx, b.cy));
                    camera.push(f);
                }
            }
        }
    }
    let pins: Vec<(usize, f32, f32)> = seeds
        .iter()
        .filter_map(|&(id, x, y)| Some((req.frames.iter().position(|&f| f == id)?, x, y)))
        .collect();
    let nearest = |p: usize| -> usize {
        let mut best = 0;
        for (i, &(sp, _, _)) in pins.iter().enumerate() {
            if sp.abs_diff(p) <= pins[best].0.abs_diff(p) {
                best = i;
            }
        }
        best
    };
    // Phase 1: positions, streamed as they come with the working-res patch
    // score so the strip re-ranks within a second.
    let points: Mutex<Vec<(usize, f32, f32, f32)>> = Mutex::new(Vec::new());
    let emit = |idx: usize, x: f32, y: f32, conf: f32, luma: &decode::Luma| {
        let (nx, ny) = (x / luma.width as f32, y / luma.height as f32);
        points.lock().unwrap().push((idx, nx, ny, conf));
        // Focus area for the coverage count: the camera's eye box when this
        // frame has one, else the search area around the point.
        let half = match (camera.contains(&idx), metas[idx].af_box_eye()) {
            (true, Some(b)) => 0.5 * (b.w * luma.width as f32).max(b.h * luma.height as f32),
            _ => req.roi_frac * luma.width.max(luma.height) as f32,
        }
        .max(8.0) as usize;
        let focus = score::Rect {
            x: (x as usize).saturating_sub(half),
            y: (y as usize).saturating_sub(half),
            w: 2 * half,
            h: 2 * half,
        };
        let _ = shared.events.send(Event::TrackPoint {
            req_id: req.req_id,
            idx,
            x: nx,
            y: ny,
            conf,
            roi_score: roi_score(luma, x, y, AREA_FRAC),
            coverage: eye::coverage(luma, focus),
        });
        shared.dirty.store(true, Ordering::Relaxed);
    };
    for (si, &(seed_pos, nx, ny)) in pins.iter().enumerate() {
        let seed_frame = req.frames[seed_pos];
        let Some(seed_luma) = track_luma(&metas[seed_frame], shared.source) else {
            continue;
        };
        let (sx, sy) = (nx * seed_luma.width as f32, ny * seed_luma.height as f32);
        emit(seed_frame, sx, sy, 1.0, &seed_luma);

        // Bidirectional wavefront from this pin, over the frames it owns.
        for dir in [1i64, -1] {
            let Some(mut tracker) = Tracker::new(&seed_luma, sx, sy) else {
                break;
            };
            let mut p = seed_pos as i64 + dir;
            while p >= 0 && (p as usize) < req.frames.len() && nearest(p as usize) == si {
                if stale() {
                    return;
                }
                let idx = req.frames[p as usize];
                let Some(luma) = track_luma(&metas[idx], shared.source) else {
                    p += dir;
                    continue;
                };
                let (x, y, conf) = tracker.step(&luma);
                emit(idx, x, y, conf, &luma);
                p += dir;
            }
        }
    }
    // Phase 2: pupil edge width at native resolution, a few frames at a time
    // (each is a ~0.25 s lossless crop of the full JPEG). Results are held
    // back until all are in: a pupil whose radius disagrees with the burst's
    // median by more than 40% is a speck or a merge, not the eye, and is
    // dropped so the frame keeps its patch score instead of a false one.
    let points = points.into_inner().unwrap();
    if points.is_empty() {
        return;
    }
    let measured: Mutex<Vec<(usize, f32, f32, RoiKind, EyeMark)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        let per = points.len().div_ceil(3).max(1);
        for chunk in points.chunks(per) {
            let (camera, req, measured) = (&camera, &req, &measured);
            s.spawn(move || {
                for &(idx, x, y, conf) in chunk {
                    if stale() {
                        return;
                    }
                    if conf < CONF_OK {
                        continue;
                    }
                    let is_cam = camera.contains(&idx);
                    if let Some((px, py, eye)) = measure_point(&metas[idx], x, y, is_cam, req.roi_frac) {
                        let kind = if is_cam { RoiKind::EyeCamera } else { RoiKind::EyeTracked };
                        measured.lock().unwrap().push((idx, px, py, kind, eye));
                    }
                }
            });
        }
    });
    if stale() {
        return;
    }
    let mut measured = measured.into_inner().unwrap();
    let mut radii: Vec<f32> = measured.iter().map(|m| m.4.r_frac).collect();
    radii.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if let Some(&median) = radii.get(radii.len() / 2) {
        measured.retain(|m| (m.4.r_frac - median).abs() <= 0.4 * median);
    }
    for (idx, x, y, kind, eye) in measured {
        let _ = shared.events.send(Event::EyePoint { req_id: req.req_id, idx, x, y, kind, eye });
    }
    let _ = shared.events.send(Event::TrackDone { req_id: req.req_id });
    shared.dirty.store(true, Ordering::Relaxed);
}

/// Pupil edge width at native resolution around a display-space point
/// (normalized). The search window is 1.5x the camera's eye box, or the
/// user's search area for tracked points. Returns the pupil centre
/// (display-space, normalized) and the measurement; None when no pupil was
/// found, in which case the frame keeps its working-resolution patch score.
fn measure_point(meta: &FileMeta, x: f32, y: f32, camera: bool, roi_frac: f32) -> Option<(f32, f32, EyeMark)> {
    let (jpeg, _, _) = formats::extract_jpeg(meta, Source::Full).ok()??;
    let (sx, sy) = decode::orient_norm(x, y, decode::inverse_orientation(meta.orientation));
    let (dw, dh) = meta.display_dims();
    let long = meta.width.max(meta.height).max(1) as f32;
    let half = match (camera, meta.af_box_eye()) {
        (true, Some(b)) => (0.75 * (b.w * dw as f32).max(b.h * dh as f32)).max(100.0),
        _ => roi_frac * long,
    } as usize;
    let (cx, cy) = ((sx * meta.width as f32) as usize, (sy * meta.height as f32) as usize);
    let (crop, x0, y0) =
        decode::decode_luma_crop(&jpeg, cx.saturating_sub(half), cy.saturating_sub(half), 2 * half, 2 * half).ok()?;
    let m = eye::measure_eye(&crop, score::Rect { x: 0, y: 0, w: crop.width, h: crop.height }, 0.5 * half as f32)?;
    if m.blob.area == 0 {
        return None; // nominal fallback circle, not a segmented pupil
    }
    let (bx, by) = ((x0 as f32 + m.blob.cx) / meta.width as f32, (y0 as f32 + m.blob.cy) / meta.height as f32);
    let (ux, uy) = decode::orient_norm(bx, by, meta.orientation);
    Some((
        ux,
        uy,
        EyeMark {
            r_frac: (m.blob.a * m.blob.b).sqrt() / long,
            width_px: m.edge.width_px,
            anisotropy: m.edge.anisotropy,
        },
    ))
}
