//! Background engine for the GUI: a scan thread plus a worker pool draining
//! a priority job queue. The UI requests thumbs/previews/scores for what is
//! visible; higher-priority jobs (the frame under the cursor) jump the
//! queue. No egui dependency — the CLI and future wasm build reuse this.

use std::collections::BinaryHeap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::cache::{file_key, Cache};
use crate::decode;
use crate::formats;
use crate::meta::FileMeta;
use crate::score;
use crate::track::Tracker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobKind {
    /// Small grid thumbnail (~200 px) from the embedded preview.
    Thumb,
    /// Full embedded preview (~1620 px) for the main view.
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
    },
}

/// A click-and-track request over one burst (frames in capture order,
/// as meta indices; seed coordinates normalized 0..1).
pub struct TrackRequest {
    pub req_id: u64,
    pub frames: Vec<usize>,
    pub seed_frame: usize,
    pub seed_x: f32,
    pub seed_y: f32,
}

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
}

pub struct Engine {
    shared: Arc<Shared>,
    pub events: crossbeam_channel::Receiver<Event>,
}

impl Engine {
    /// Spawn the scan for `dir` plus `workers` decode threads. Score results
    /// are cached in `cache_path`.
    pub fn start(dir: PathBuf, cache_path: PathBuf, workers: usize) -> Engine {
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
                    run_track(&shared, &metas, track);
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
        match job.kind {
            JobKind::Thumb | JobKind::Preview => {
                let min = if job.kind == JobKind::Thumb { 200 } else { 1600 };
                if let Ok(Some((jpeg, _, _))) = formats::extract_preview(meta) {
                    if let Ok(img) = decode::decode_rgba_scaled(&jpeg, min) {
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
            JobKind::Full => {
                if let Some(info) = meta.fullsize.or(meta.preview) {
                    if let Ok(mut r) = crate::io::FileReader::open(&meta.path) {
                        use crate::io::ReadRange;
                        if let Ok(jpeg) = r.read_range(info.range.offset, info.range.len as usize)
                        {
                            if let Ok(img) = decode::decode_rgba_scaled(&jpeg, usize::MAX / 2) {
                                let _ = shared.events.send(Event::Image {
                                    kind: JobKind::Full,
                                    idx: job.idx,
                                    rgba: img.pixels,
                                    width: img.width,
                                    height: img.height,
                                });
                            }
                        }
                    }
                }
            }
            JobKind::Score => {
                let key = file_key(meta);
                let cached = {
                    let cache = shared.cache.lock().unwrap();
                    cache.as_ref().and_then(|c| c.get_score(&key))
                };
                let s = cached.or_else(|| {
                    let (jpeg, _, _) = formats::extract_preview(meta).ok()??;
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
        shared.dirty.store(true, Ordering::Relaxed);
    }
}

fn track_luma(meta: &FileMeta) -> Option<decode::Luma> {
    let (jpeg, _, _) = formats::extract_preview(meta).ok()??;
    decode::decode_luma_scaled(&jpeg, 1600).ok()
}

/// Sharpness in a patch centered on the tracked point.
fn roi_score(luma: &decode::Luma, x: f32, y: f32) -> f32 {
    const R: usize = 96;
    let cx = (x as usize).clamp(R, luma.width.saturating_sub(R));
    let cy = (y as usize).clamp(R, luma.height.saturating_sub(R));
    score::score_region(
        luma,
        score::Rect {
            x: cx - R,
            y: cy - R,
            w: 2 * R,
            h: 2 * R,
        },
    )
    .score
}

fn run_track(shared: &Shared, metas: &[FileMeta], req: TrackRequest) {
    let stale = || shared.latest_track.load(Ordering::Relaxed) != req.req_id;
    let Some(seed_pos) = req.frames.iter().position(|&f| f == req.seed_frame) else {
        return;
    };
    let Some(seed_luma) = track_luma(&metas[req.seed_frame]) else {
        return;
    };
    let (sx, sy) = (
        req.seed_x * seed_luma.width as f32,
        req.seed_y * seed_luma.height as f32,
    );
    let emit = |idx: usize, x: f32, y: f32, conf: f32, luma: &decode::Luma| {
        let _ = shared.events.send(Event::TrackPoint {
            req_id: req.req_id,
            idx,
            x: x / luma.width as f32,
            y: y / luma.height as f32,
            conf,
            roi_score: roi_score(luma, x, y),
        });
        shared.dirty.store(true, Ordering::Relaxed);
    };
    emit(req.seed_frame, sx, sy, 1.0, &seed_luma);

    // Bidirectional wavefront from the seed.
    for dir in [1i64, -1] {
        let Some(mut tracker) = Tracker::new(&seed_luma, sx, sy) else {
            return;
        };
        let mut p = seed_pos as i64 + dir;
        while p >= 0 && (p as usize) < req.frames.len() {
            if stale() {
                return;
            }
            let idx = req.frames[p as usize];
            let Some(luma) = track_luma(&metas[idx]) else {
                p += dir;
                continue;
            };
            let (x, y, conf) = tracker.step(&luma);
            emit(idx, x, y, conf, &luma);
            p += dir;
        }
    }
}
