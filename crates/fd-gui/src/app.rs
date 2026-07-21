use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Key, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};
use fd_core::burst::{self, Burst, LogicalImage};
use fd_core::meta::FileMeta;
use fd_core::pipeline::{Engine, Event, JobKind, TrackRequest};

const AUTO_PICK_N: usize = 2;
const THUMB_BUDGET: usize = 1500;
const PREVIEW_BUDGET: usize = 8;
const FULL_BUDGET: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Flag {
    #[default]
    Unrated,
    Picked,
    Rejected,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImgState {
    pub flag: Flag,
    pub rating: u8,
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Overview,
    Burst { b: usize, frame: usize },
}

#[derive(Clone, Copy, PartialEq)]
enum SortMode {
    Time,
    Sharpness,
}

struct Tex {
    handle: TextureHandle,
    last_used: u64,
}

/// Click-and-track state for one burst.
#[derive(Default)]
struct RoiTrack {
    req_id: u64,
    /// meta idx -> (x, y, confidence), normalized coordinates.
    points: HashMap<usize, (f32, f32, f32)>,
    /// meta idx -> sharpness at the tracked point.
    roi_scores: HashMap<usize, f32>,
}

const CONF_OK: f32 = 0.55;

struct HarvestUi {
    open: bool,
    do_xmp: bool,
    do_copy: bool,
    dest: String,
    progress: Option<mpsc::Receiver<HarvestMsg>>,
    status: String,
}

enum HarvestMsg {
    Progress(usize, usize),
    Done(String),
}

pub struct App {
    dir: PathBuf,
    engine: Engine,
    metas: Vec<FileMeta>,
    /// Logical images (RAW+JPEG paired); id = primary meta index.
    images: Vec<LogicalImage>,
    bursts: Vec<Burst>,
    /// Per logical image (keyed by primary meta idx).
    state: HashMap<usize, ImgState>,
    scores: HashMap<usize, f32>,
    roi: HashMap<usize, RoiTrack>,
    track_burst: HashMap<u64, usize>,
    next_track_id: u64,
    undo: Vec<Vec<(usize, ImgState)>>,
    view: View,
    overview_cursor: usize,
    sort: SortMode,
    zoom_100: bool,
    pan: Vec2,
    textures: HashMap<(JobKind, usize), Tex>,
    requested: HashSet<(JobKind, usize)>,
    frame_no: u64,
    scan_done: bool,
    scan_started: Instant,
    scores_expected: usize,
    harvest: HarvestUi,
    show_help: bool,
    session_dirty: bool,
    last_save: Instant,
    // screenshot self-test
    screenshot: Option<PathBuf>,
    shot_frames: u64,
    open_burst: Option<usize>,
    auto_track: Option<(f32, f32)>,
}

impl App {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        dir: PathBuf,
        screenshot: Option<PathBuf>,
        shot_frames: u64,
        open_burst: Option<usize>,
        auto_track: Option<(f32, f32)>,
    ) -> Self {
        let workers = std::thread::available_parallelism()
            .map(|v| v.get().saturating_sub(2).max(2))
            .unwrap_or(4);
        let engine = Engine::start(dir.clone(), dir.join(".fd-cache.db"), workers);
        App {
            dir,
            engine,
            metas: Vec::new(),
            images: Vec::new(),
            bursts: Vec::new(),
            state: HashMap::new(),
            scores: HashMap::new(),
            roi: HashMap::new(),
            track_burst: HashMap::new(),
            next_track_id: 0,
            undo: Vec::new(),
            view: View::Overview,
            overview_cursor: 0,
            sort: SortMode::Sharpness,
            zoom_100: false,
            pan: Vec2::ZERO,
            textures: HashMap::new(),
            requested: HashSet::new(),
            frame_no: 0,
            scan_done: false,
            scan_started: Instant::now(),
            scores_expected: 0,
            harvest: HarvestUi {
                open: false,
                do_xmp: true,
                do_copy: false,
                dest: String::new(),
                progress: None,
                status: String::new(),
            },
            show_help: false,
            session_dirty: false,
            last_save: Instant::now(),
            screenshot,
            shot_frames,
            open_burst,
            auto_track,
        }
    }

    // ---------- engine plumbing ----------

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.engine.events.try_recv() {
            match ev {
                Event::Scanned { metas, .. } => {
                    self.metas = metas;
                    let images = burst::pair(&self.metas);
                    self.bursts = burst::group(&self.metas, images.clone(), 60);
                    self.images = images;
                    self.scan_done = true;
                    self.load_session();
                    if let Some(b) = self.open_burst.take() {
                        if !self.bursts.is_empty() {
                            let b = b.min(self.bursts.len() - 1);
                            self.enter_burst(b);
                            if let Some((nx, ny)) = self.auto_track.take() {
                                let seed = self.bursts[b].images[0].primary();
                                self.start_track(b, seed, nx, ny);
                            }
                        }
                    }
                    // background scoring for every logical image
                    self.scores_expected = 0;
                    for b in &self.bursts {
                        for li in &b.images {
                            self.engine.request(JobKind::Score, li.primary(), 10);
                            self.scores_expected += 1;
                        }
                    }
                }
                Event::Image {
                    kind,
                    idx,
                    rgba,
                    width,
                    height,
                } => {
                    let img = ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                    let handle = ctx.load_texture(
                        format!("{kind:?}{idx}"),
                        img,
                        TextureOptions::LINEAR,
                    );
                    self.textures.insert(
                        (kind, idx),
                        Tex {
                            handle,
                            last_used: self.frame_no,
                        },
                    );
                    self.evict(kind);
                }
                Event::Score { idx, score } => {
                    self.scores.insert(idx, score);
                }
                Event::TrackPoint {
                    req_id,
                    idx,
                    x,
                    y,
                    conf,
                    roi_score,
                } => {
                    if let Some(&b) = self.track_burst.get(&req_id) {
                        let entry = self.roi.entry(b).or_default();
                        if entry.req_id == req_id {
                            entry.points.insert(idx, (x, y, conf));
                            entry.roi_scores.insert(idx, roi_score);
                        }
                    }
                }
            }
        }
    }

    fn evict(&mut self, kind: JobKind) {
        let budget = match kind {
            JobKind::Thumb => THUMB_BUDGET,
            JobKind::Preview => PREVIEW_BUDGET,
            JobKind::Full => FULL_BUDGET,
            JobKind::Score => return,
        };
        let count = self.textures.keys().filter(|(k, _)| *k == kind).count();
        if count <= budget {
            return;
        }
        let mut of_kind: Vec<(usize, u64)> = self
            .textures
            .iter()
            .filter(|((k, _), _)| *k == kind)
            .map(|((_, i), t)| (*i, t.last_used))
            .collect();
        of_kind.sort_by_key(|(_, last)| *last);
        for (idx, _) in of_kind.into_iter().take(count - budget) {
            self.textures.remove(&(kind, idx));
            self.requested.remove(&(kind, idx));
        }
    }

    /// Get a texture, requesting the decode if missing. Marks LRU use.
    fn tex(&mut self, kind: JobKind, idx: usize, priority: i32) -> Option<egui::TextureId> {
        if let Some(t) = self.textures.get_mut(&(kind, idx)) {
            t.last_used = self.frame_no;
            return Some(t.handle.id());
        }
        if self.requested.insert((kind, idx)) {
            self.engine.request(kind, idx, priority);
        }
        None
    }

    // ---------- state / selection ----------

    fn img_state(&self, id: usize) -> ImgState {
        self.state.get(&id).copied().unwrap_or_default()
    }

    fn apply(&mut self, changes: Vec<(usize, ImgState)>) {
        let prev: Vec<(usize, ImgState)> =
            changes.iter().map(|(id, _)| (*id, self.img_state(*id))).collect();
        self.undo.push(prev);
        for (id, st) in changes {
            if st == ImgState::default() {
                self.state.remove(&id);
            } else {
                self.state.insert(id, st);
            }
        }
        self.session_dirty = true;
    }

    fn undo_last(&mut self) {
        if let Some(prev) = self.undo.pop() {
            for (id, st) in prev {
                if st == ImgState::default() {
                    self.state.remove(&id);
                } else {
                    self.state.insert(id, st);
                }
            }
            self.session_dirty = true;
        }
    }

    fn burst_culled(&self, b: &Burst) -> bool {
        b.images
            .iter()
            .all(|li| self.img_state(li.primary()).flag != Flag::Unrated)
    }

    /// Ranking key: ROI sharpness when a confident track exists (tracked
    /// frames always outrank lost/untracked ones), else global sharpness.
    fn effective_score(&self, b: usize, id: usize) -> (bool, f32) {
        if let Some(t) = self.roi.get(&b) {
            if let (Some(&(_, _, conf)), Some(&rs)) =
                (t.points.get(&id), t.roi_scores.get(&id))
            {
                if conf >= CONF_OK {
                    return (true, rs);
                }
                return (false, self.scores.get(&id).copied().unwrap_or(0.0));
            }
        }
        (
            self.roi.get(&b).is_none(),
            self.scores.get(&id).copied().unwrap_or(0.0),
        )
    }

    fn rank_by_score(&self, b: usize, idxs: &mut Vec<usize>) {
        let burst = &self.bursts[b];
        idxs.sort_by(|&x, &y| {
            let (tx, sx) = self.effective_score(b, burst.images[x].primary());
            let (ty, sy) = self.effective_score(b, burst.images[y].primary());
            ty.cmp(&tx)
                .then(sy.partial_cmp(&sx).unwrap_or(std::cmp::Ordering::Equal))
        });
    }

    /// Frame display order for a burst under the current sort mode.
    fn order(&self, b: usize) -> Vec<usize> {
        let mut idxs: Vec<usize> = (0..self.bursts[b].images.len()).collect();
        if self.sort == SortMode::Sharpness {
            self.rank_by_score(b, &mut idxs);
        }
        idxs
    }

    fn accept_burst(&mut self, b: usize) {
        let burst = &self.bursts[b];
        let mut changes = Vec::new();
        // Top-N by sharpness regardless of current sort mode.
        let mut by_score: Vec<usize> = (0..burst.images.len()).collect();
        self.rank_by_score(b, &mut by_score);
        let burst = &self.bursts[b];
        for (rank, &fi) in by_score.iter().enumerate() {
            let id = burst.images[fi].primary();
            let mut st = self.img_state(id);
            st.flag = if rank < AUTO_PICK_N {
                Flag::Picked
            } else {
                Flag::Rejected
            };
            changes.push((id, st));
        }
        self.apply(changes);
    }

    fn next_unculled(&self, from: usize) -> Option<usize> {
        (1..=self.bursts.len())
            .map(|d| (from + d) % self.bursts.len())
            .find(|&b| !self.burst_culled(&self.bursts[b]))
    }

    // ---------- session persistence ----------

    fn session_path(&self) -> PathBuf {
        self.dir.join(".fd-session.tsv")
    }

    fn load_session(&mut self) {
        let Ok(text) = std::fs::read_to_string(self.session_path()) else {
            return;
        };
        let by_path: HashMap<&str, usize> = self
            .images
            .iter()
            .map(|li| {
                (
                    self.metas[li.primary()].path.to_str().unwrap_or(""),
                    li.primary(),
                )
            })
            .collect();
        for line in text.lines() {
            let mut parts = line.split('\t');
            let (Some(path), Some(flag), Some(rating)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if let Some(&id) = by_path.get(path) {
                let flag = match flag {
                    "P" => Flag::Picked,
                    "X" => Flag::Rejected,
                    _ => Flag::Unrated,
                };
                let rating: u8 = rating.parse().unwrap_or(0);
                if flag != Flag::Unrated || rating > 0 {
                    self.state.insert(id, ImgState { flag, rating });
                }
            }
        }
    }

    fn save_session(&mut self) {
        let mut out = String::new();
        for (&id, st) in &self.state {
            let flag = match st.flag {
                Flag::Picked => "P",
                Flag::Rejected => "X",
                Flag::Unrated => "U",
            };
            if let Some(p) = self.metas.get(id).and_then(|m| m.path.to_str()) {
                out.push_str(&format!("{p}\t{flag}\t{}\n", st.rating));
            }
        }
        let _ = std::fs::write(self.session_path(), out);
        self.session_dirty = false;
        self.last_save = Instant::now();
    }

    // ---------- input ----------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (enter, esc, ctrl) = ctx.input(|i| {
            (
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
                i.modifiers.command,
            )
        });
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));

        if pressed(Key::Questionmark) || pressed(Key::H) && !ctrl {
            self.show_help = !self.show_help;
        }
        if ctrl && pressed(Key::H) {
            self.harvest.open = !self.harvest.open;
        }
        if ctrl && pressed(Key::Z) {
            self.undo_last();
        }

        match self.view {
            View::Overview => {
                let cols = self.overview_cols(ctx);
                let n = self.bursts.len();
                if n == 0 {
                    return;
                }
                let c = &mut self.overview_cursor;
                if pressed(Key::ArrowRight) {
                    *c = (*c + 1).min(n - 1);
                }
                if pressed(Key::ArrowLeft) {
                    *c = c.saturating_sub(1);
                }
                if pressed(Key::ArrowDown) {
                    *c = (*c + cols).min(n - 1);
                }
                if pressed(Key::ArrowUp) {
                    *c = c.saturating_sub(cols);
                }
                if enter {
                    self.enter_burst(self.overview_cursor);
                }
                if pressed(Key::N) {
                    if let Some(b) = self.next_unculled(self.overview_cursor) {
                        self.overview_cursor = b;
                    }
                }
            }
            View::Burst { b, frame } => {
                let order = self.order(b);
                let nf = order.len();
                let mut new_frame = frame;
                if pressed(Key::ArrowRight) {
                    new_frame = (frame + 1).min(nf - 1);
                }
                if pressed(Key::ArrowLeft) {
                    new_frame = frame.saturating_sub(1);
                }
                if pressed(Key::ArrowDown) && b + 1 < self.bursts.len() {
                    self.view = View::Burst { b: b + 1, frame: 0 };
                    self.zoom_100 = false;
                    return;
                }
                if pressed(Key::ArrowUp) && b > 0 {
                    self.view = View::Burst { b: b - 1, frame: 0 };
                    self.zoom_100 = false;
                    return;
                }
                if pressed(Key::N) {
                    if let Some(nb) = self.next_unculled(b) {
                        self.view = View::Burst { b: nb, frame: 0 };
                        self.zoom_100 = false;
                        return;
                    }
                }
                if esc {
                    self.overview_cursor = b;
                    self.view = View::Overview;
                    return;
                }
                if pressed(Key::Z) && !ctrl {
                    self.zoom_100 = !self.zoom_100;
                    self.pan = Vec2::ZERO;
                }
                if pressed(Key::O) {
                    self.sort = match self.sort {
                        SortMode::Time => SortMode::Sharpness,
                        SortMode::Sharpness => SortMode::Time,
                    };
                }

                let id = self.bursts[b].images[order[frame]].primary();
                let mut st = self.img_state(id);
                let mut changed = false;
                let mut advance = false;
                if pressed(Key::P) {
                    st.flag = Flag::Picked;
                    changed = true;
                    advance = true;
                }
                if pressed(Key::X) && !ctrl {
                    st.flag = Flag::Rejected;
                    changed = true;
                    advance = true;
                }
                if pressed(Key::U) {
                    st.flag = Flag::Unrated;
                    changed = true;
                }
                for (key, r) in [
                    (Key::Num0, 0u8),
                    (Key::Num1, 1),
                    (Key::Num2, 2),
                    (Key::Num3, 3),
                    (Key::Num4, 4),
                    (Key::Num5, 5),
                ] {
                    if pressed(key) {
                        st.rating = r;
                        changed = true;
                    }
                }
                if changed {
                    self.apply(vec![(id, st)]);
                }
                if advance && new_frame == frame {
                    new_frame = (frame + 1).min(nf - 1);
                }

                if ctrl && enter {
                    self.accept_burst(b);
                    if let Some(nb) = self.next_unculled(b) {
                        self.view = View::Burst { b: nb, frame: 0 };
                        self.zoom_100 = false;
                        return;
                    }
                }
                if ctrl && pressed(Key::X) {
                    let burst = &self.bursts[b];
                    let changes: Vec<(usize, ImgState)> = burst
                        .images
                        .iter()
                        .map(|li| {
                            let id = li.primary();
                            let mut s = self.img_state(id);
                            s.flag = Flag::Rejected;
                            (id, s)
                        })
                        .collect();
                    self.apply(changes);
                }

                if new_frame != frame {
                    self.view = View::Burst { b, frame: new_frame };
                    self.pan = Vec2::ZERO;
                }
            }
        }
    }

    fn start_track(&mut self, b: usize, seed_id: usize, nx: f32, ny: f32) {
        self.next_track_id += 1;
        let req_id = self.next_track_id;
        let frames: Vec<usize> = self.bursts[b].images.iter().map(|li| li.primary()).collect();
        self.track_burst.insert(req_id, b);
        self.roi.insert(
            b,
            RoiTrack {
                req_id,
                points: HashMap::new(),
                roi_scores: HashMap::new(),
            },
        );
        self.engine.request_track(TrackRequest {
            req_id,
            frames,
            seed_frame: seed_id,
            seed_x: nx,
            seed_y: ny,
        });
    }

    fn enter_burst(&mut self, b: usize) {
        self.view = View::Burst { b, frame: 0 };
        self.zoom_100 = false;
        self.pan = Vec2::ZERO;
    }

    fn overview_cols(&self, ctx: &egui::Context) -> usize {
        let w = ctx.screen_rect().width() - 24.0;
        ((w / 176.0).floor() as usize).max(1)
    }

    // ---------- drawing ----------

    fn draw_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if !self.scan_done {
                ui.spinner();
                ui.label(format!(
                    "scanning {} … {:.1}s",
                    self.dir.display(),
                    self.scan_started.elapsed().as_secs_f32()
                ));
                return;
            }
            let picks = self
                .state
                .values()
                .filter(|s| s.flag == Flag::Picked)
                .count();
            let rejects = self
                .state
                .values()
                .filter(|s| s.flag == Flag::Rejected)
                .count();
            let culled = self
                .bursts
                .iter()
                .filter(|b| self.burst_culled(b))
                .count();
            ui.label(format!(
                "{} images · {} bursts · {}/{} culled · {} picks · {} rejects",
                self.images.len(),
                self.bursts.len(),
                culled,
                self.bursts.len(),
                picks,
                rejects
            ));
            if self.scores.len() < self.scores_expected {
                ui.spinner();
                ui.label(format!(
                    "scoring {}/{}",
                    self.scores.len(),
                    self.scores_expected
                ));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Harvest (Ctrl+H)").clicked() {
                    self.harvest.open = true;
                }
                if ui.button("Help (?)").clicked() {
                    self.show_help = !self.show_help;
                }
                let sort = match self.sort {
                    SortMode::Time => "sort: time (O)",
                    SortMode::Sharpness => "sort: sharpness (O)",
                };
                if ui.button(sort).clicked() {
                    self.sort = match self.sort {
                        SortMode::Time => SortMode::Sharpness,
                        SortMode::Sharpness => SortMode::Time,
                    };
                }
            });
        });
    }

    fn draw_overview(&mut self, ui: &mut egui::Ui) {
        let cols = self.overview_cols(ui.ctx());
        let cell = Vec2::new(168.0, 150.0);
        let rows = self.bursts.len().div_ceil(cols);
        let cursor = self.overview_cursor;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, cell.y, rows, |ui, range| {
                for row in range {
                    ui.horizontal(|ui| {
                        for col in 0..cols {
                            let b = row * cols + col;
                            if b >= self.bursts.len() {
                                break;
                            }
                            self.draw_burst_cell(ui, b, cell, b == cursor);
                        }
                    });
                }
            });
    }

    fn draw_burst_cell(&mut self, ui: &mut egui::Ui, b: usize, cell: Vec2, selected: bool) {
        let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
        if !ui.is_rect_visible(rect) {
            return;
        }
        if resp.double_clicked() || (selected && resp.clicked()) {
            self.enter_burst(b);
        } else if resp.clicked() {
            self.overview_cursor = b;
        }

        // cover = best-scored frame (or first)
        let order = self.order(b);
        let cover_id = self.bursts[b].images[order[0]].primary();
        let n = self.bursts[b].images.len();
        let culled = self.burst_culled(&self.bursts[b]);
        let picks = self.bursts[b]
            .images
            .iter()
            .filter(|li| self.img_state(li.primary()).flag == Flag::Picked)
            .count();

        let img_rect = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(160.0, 110.0));
        let painter = ui.painter();
        painter.rect_filled(img_rect, 4.0, Color32::from_gray(28));
        // stack effect for real bursts
        if n > 1 {
            let back = img_rect.translate(Vec2::new(3.0, -3.0));
            painter.rect_stroke(
                back,
                4.0,
                Stroke::new(1.0, Color32::from_gray(70)),
                egui::StrokeKind::Outside,
            );
        }
        if let Some(tid) = self.tex(JobKind::Thumb, cover_id, 50) {
            ui.painter().image(
                tid,
                img_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let painter = ui.painter();
        // badges
        painter.text(
            img_rect.right_top() + Vec2::new(-4.0, 4.0),
            Align2::RIGHT_TOP,
            format!("{n}"),
            FontId::proportional(13.0),
            Color32::WHITE,
        );
        let ring = if culled {
            Color32::from_rgb(80, 200, 90)
        } else if picks > 0 {
            Color32::from_rgb(230, 180, 60)
        } else {
            Color32::from_gray(70)
        };
        painter.rect_stroke(
            img_rect,
            4.0,
            Stroke::new(if selected { 3.0 } else { 1.5 }, if selected { Color32::from_rgb(90, 160, 255) } else { ring }),
            egui::StrokeKind::Outside,
        );
        let name = self.metas[cover_id]
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let label = if picks > 0 {
            format!("{name} · {picks}P")
        } else {
            name
        };
        painter.text(
            egui::pos2(rect.min.x + 6.0, img_rect.max.y + 4.0),
            Align2::LEFT_TOP,
            label,
            FontId::proportional(12.0),
            Color32::from_gray(190),
        );
    }

    fn draw_burst_view(&mut self, ui: &mut egui::Ui, b: usize, frame: usize) {
        let order = self.order(b);
        let frame = frame.min(order.len().saturating_sub(1));
        let id = self.bursts[b].images[order[frame]].primary();

        let strip_h = 132.0;
        let avail = ui.available_size();
        let main_h = (avail.y - strip_h).max(100.0);

        // ---- main image ----
        let (main_rect, main_resp) = ui.allocate_exact_size(
            Vec2::new(avail.x, main_h),
            Sense::click_and_drag(),
        );
        ui.painter().rect_filled(main_rect, 0.0, Color32::from_gray(12));

        let full_tid = self.textures.get(&(JobKind::Full, id)).map(|t| t.handle.id());
        let (tid, native) = if self.zoom_100 {
            self.tex(JobKind::Full, id, 100);
            let t = self
                .textures
                .get_mut(&(JobKind::Full, id))
                .map(|t| {
                    t.last_used = self.frame_no;
                    (t.handle.id(), t.handle.size_vec2())
                });
            let dims = Vec2::new(
                self.metas[id].width.max(1) as f32,
                self.metas[id].height.max(1) as f32,
            );
            match t {
                Some((tid, _)) => (Some(tid), dims),
                None => (self.tex(JobKind::Preview, id, 90), dims),
            }
        } else {
            let tid = match full_tid {
                Some(t) => Some(t),
                None => self.tex(JobKind::Preview, id, 90),
            };
            (tid, Vec2::ZERO)
        };

        let mut display_rect: Option<Rect> = None;
        if let Some(tid) = tid {
            if self.zoom_100 {
                if main_resp.dragged() {
                    self.pan += main_resp.drag_delta();
                }
                // 1:1 pixels around center + pan
                let size = native;
                let min = main_rect.center() - size * 0.5 + self.pan;
                let img_rect = Rect::from_min_size(min.round(), size);
                ui.painter().with_clip_rect(main_rect).image(
                    tid,
                    img_rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                display_rect = Some(img_rect);
                let chip = if self.textures.contains_key(&(JobKind::Full, id)) {
                    "FULL"
                } else {
                    "PREVIEW"
                };
                ui.painter().text(
                    main_rect.left_top() + Vec2::new(8.0, 8.0),
                    Align2::LEFT_TOP,
                    format!("100% · {chip}"),
                    FontId::proportional(13.0),
                    Color32::from_rgb(230, 180, 60),
                );
            } else {
                // fit
                let tex_size = self
                    .textures
                    .iter()
                    .find(|((k, i), _)| (*k == JobKind::Full || *k == JobKind::Preview) && *i == id)
                    .map(|(_, t)| t.handle.size_vec2())
                    .unwrap_or(Vec2::new(1620.0, 1080.0));
                let scale = (main_rect.width() / tex_size.x)
                    .min(main_rect.height() / tex_size.y)
                    .min(4.0);
                let size = tex_size * scale;
                let img_rect = Rect::from_center_size(main_rect.center(), size);
                ui.painter().image(
                    tid,
                    img_rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                display_rect = Some(img_rect);
            }
        } else {
            ui.painter().text(
                main_rect.center(),
                Align2::CENTER_CENTER,
                "loading…",
                FontId::proportional(16.0),
                Color32::from_gray(120),
            );
        }

        // Click on the image = set the tracking point (e.g. the eye).
        if let (Some(img_rect), true) = (display_rect, main_resp.clicked()) {
            if let Some(pos) = main_resp.interact_pointer_pos() {
                let nx = (pos.x - img_rect.min.x) / img_rect.width();
                let ny = (pos.y - img_rect.min.y) / img_rect.height();
                if (0.0..=1.0).contains(&nx) && (0.0..=1.0).contains(&ny) {
                    self.start_track(b, id, nx, ny);
                }
            }
        }

        // ROI overlay on the main image.
        if let Some((img_rect, &(x, y, conf))) = display_rect
            .zip(self.roi.get(&b).and_then(|t| t.points.get(&id)))
        {
            let center = egui::pos2(
                img_rect.min.x + x * img_rect.width(),
                img_rect.min.y + y * img_rect.height(),
            );
            let side = (64.0 / 1620.0) * img_rect.width();
            let color = if conf >= 0.75 {
                Color32::from_rgb(80, 200, 90)
            } else if conf >= CONF_OK {
                Color32::from_rgb(230, 180, 60)
            } else {
                Color32::from_rgb(220, 70, 70)
            };
            ui.painter().with_clip_rect(main_rect).rect_stroke(
                Rect::from_center_size(center, Vec2::splat(side)),
                2.0,
                Stroke::new(2.0, color),
                egui::StrokeKind::Outside,
            );
        }

        // overlay: filename + score + state
        let st = self.img_state(id);
        let score_txt = match self.roi.get(&b).and_then(|t| t.roi_scores.get(&id)) {
            Some(rs) => format!("ROI {rs:.1}"),
            None => self
                .scores
                .get(&id)
                .map(|s| format!("{s:.1}"))
                .unwrap_or_else(|| "…".into()),
        };
        let flag_txt = match st.flag {
            Flag::Picked => " · PICK",
            Flag::Rejected => " · REJECT",
            Flag::Unrated => "",
        };
        let stars = "★".repeat(st.rating as usize);
        ui.painter().text(
            main_rect.left_bottom() + Vec2::new(8.0, -8.0),
            Align2::LEFT_BOTTOM,
            format!(
                "burst {}/{} · frame {}/{} · {} · sharp {score_txt}{flag_txt} {stars}",
                b + 1,
                self.bursts.len(),
                frame + 1,
                order.len(),
                self.metas[id]
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            ),
            FontId::proportional(14.0),
            Color32::WHITE,
        );

        // ---- filmstrip ----
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (pos, &fi) in order.iter().enumerate() {
                        let fid = self.bursts[b].images[fi].primary();
                        let cell = Vec2::new(140.0, 118.0);
                        let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
                        if resp.clicked() {
                            self.view = View::Burst { b, frame: pos };
                            self.pan = Vec2::ZERO;
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let img_rect =
                            Rect::from_min_size(rect.min + Vec2::new(2.0, 2.0), Vec2::new(136.0, 92.0));
                        ui.painter().rect_filled(img_rect, 3.0, Color32::from_gray(25));
                        if let Some(tid) = self.tex(JobKind::Thumb, fid, 60) {
                            let sst = self.img_state(fid);
                            let tint = if sst.flag == Flag::Rejected {
                                Color32::from_gray(110)
                            } else {
                                Color32::WHITE
                            };
                            ui.painter().image(
                                tid,
                                img_rect,
                                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                tint,
                            );
                        }
                        // mini ROI box with confidence color
                        if let Some(&(x, y, conf)) =
                            self.roi.get(&b).and_then(|t| t.points.get(&fid))
                        {
                            let c = egui::pos2(
                                img_rect.min.x + x * img_rect.width(),
                                img_rect.min.y + y * img_rect.height(),
                            );
                            let color = if conf >= 0.75 {
                                Color32::from_rgb(80, 200, 90)
                            } else if conf >= CONF_OK {
                                Color32::from_rgb(230, 180, 60)
                            } else {
                                Color32::from_rgb(220, 70, 70)
                            };
                            ui.painter().with_clip_rect(img_rect).rect_stroke(
                                Rect::from_center_size(c, Vec2::splat(7.0)),
                                1.0,
                                Stroke::new(1.5, color),
                                egui::StrokeKind::Outside,
                            );
                        }
                        let sst = self.img_state(fid);
                        let border = if pos == frame {
                            Stroke::new(3.0, Color32::from_rgb(90, 160, 255))
                        } else {
                            match sst.flag {
                                Flag::Picked => Stroke::new(2.0, Color32::from_rgb(80, 200, 90)),
                                Flag::Rejected => Stroke::new(2.0, Color32::from_rgb(200, 70, 70)),
                                Flag::Unrated => Stroke::new(1.0, Color32::from_gray(70)),
                            }
                        };
                        ui.painter()
                            .rect_stroke(img_rect, 3.0, border, egui::StrokeKind::Outside);
                        let s = match self.roi.get(&b).and_then(|t| t.roi_scores.get(&fid)) {
                            Some(rs) => format!("•{rs:.1}"),
                            None => self
                                .scores
                                .get(&fid)
                                .map(|s| format!("{s:.1}"))
                                .unwrap_or_else(|| "…".into()),
                        };
                        let flag_c = match sst.flag {
                            Flag::Picked => "P ",
                            Flag::Rejected => "X ",
                            Flag::Unrated => "",
                        };
                        ui.painter().text(
                            egui::pos2(rect.min.x + 4.0, img_rect.max.y + 2.0),
                            Align2::LEFT_TOP,
                            format!("{flag_c}{s} {}", "★".repeat(sst.rating as usize)),
                            FontId::proportional(11.0),
                            Color32::from_gray(200),
                        );
                    }
                });
            });

        // prefetch neighbors' previews
        for d in [1usize, 2] {
            if frame + d < order.len() {
                let nid = self.bursts[b].images[order[frame + d]].primary();
                self.tex(JobKind::Preview, nid, 80 - d as i32);
            }
            if frame >= d {
                let nid = self.bursts[b].images[order[frame - d]].primary();
                self.tex(JobKind::Preview, nid, 80 - d as i32);
            }
        }
    }

    fn draw_harvest(&mut self, ctx: &egui::Context) {
        if !self.harvest.open {
            return;
        }
        let picks: Vec<usize> = self
            .images
            .iter()
            .map(|li| li.primary())
            .filter(|id| self.img_state(*id).flag == Flag::Picked)
            .collect();
        let mut open = self.harvest.open;
        egui::Window::new("Harvest")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!("{} picked images", picks.len()));
                ui.checkbox(&mut self.harvest.do_xmp, "Write XMP sidecars (rating)");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.harvest.do_copy, "Copy picks to:");
                    ui.text_edit_singleline(&mut self.harvest.dest);
                });
                ui.add_space(6.0);

                if let Some(rx) = self.harvest.progress.take() {
                    let mut finished = false;
                    while let Ok(msg) = rx.try_recv() {
                        match msg {
                            HarvestMsg::Progress(done, total) => {
                                self.harvest.status = format!("{done}/{total}…");
                            }
                            HarvestMsg::Done(s) => {
                                self.harvest.status = s;
                                finished = true;
                            }
                        }
                    }
                    if !finished {
                        self.harvest.progress = Some(rx);
                        ui.spinner();
                    }
                }
                if !self.harvest.status.is_empty() {
                    ui.label(self.harvest.status.clone());
                }

                let busy = self.harvest.progress.is_some();
                let runnable = !busy
                    && !picks.is_empty()
                    && (self.harvest.do_xmp || (self.harvest.do_copy && !self.harvest.dest.is_empty()));
                if ui.add_enabled(runnable, egui::Button::new("Run")).clicked() {
                    let jobs: Vec<(PathBuf, Option<PathBuf>, u8)> = picks
                        .iter()
                        .map(|&id| {
                            let li = self
                                .images
                                .iter()
                                .find(|li| li.primary() == id)
                                .cloned()
                                .unwrap();
                            let jpeg_path = li
                                .jpeg
                                .filter(|&j| Some(j) != li.raw)
                                .map(|j| self.metas[j].path.clone());
                            let rating = self.img_state(id).rating.max(3);
                            (self.metas[id].path.clone(), jpeg_path, rating)
                        })
                        .collect();
                    let do_xmp = self.harvest.do_xmp;
                    let dest = self
                        .harvest
                        .do_copy
                        .then(|| PathBuf::from(self.harvest.dest.clone()));
                    let (tx, rx) = mpsc::channel();
                    self.harvest.progress = Some(rx);
                    self.harvest.status.clear();
                    std::thread::spawn(move || {
                        let total = jobs.len();
                        let mut copied = 0usize;
                        let mut sidecars = 0usize;
                        for (i, (raw, jpeg, rating)) in jobs.iter().enumerate() {
                            if do_xmp {
                                if fd_core::output::write_sidecar(raw, *rating, None).is_ok() {
                                    sidecars += 1;
                                }
                            }
                            if let Some(dest) = &dest {
                                if fd_core::output::copy_pick(raw, dest).is_ok() {
                                    copied += 1;
                                }
                                if let Some(j) = jpeg {
                                    let _ = fd_core::output::copy_pick(j, dest);
                                }
                            }
                            let _ = tx.send(HarvestMsg::Progress(i + 1, total));
                        }
                        let _ = tx.send(HarvestMsg::Done(format!(
                            "done: {sidecars} sidecars, {copied} copied"
                        )));
                    });
                }
            });
        self.harvest.open = open;
    }

    fn draw_help(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        egui::Window::new("Keyboard")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.monospace(
                    "Overview   arrows move · Enter open burst · N next unculled\n\
                     Burst      ←/→ frame · ↑/↓ burst · Esc back · Z zoom 100%\n\
                     Track      click the subject (e.g. the eye) — the point is tracked\n\
                                through the burst and frames re-rank by sharpness there;\n\
                                box color = confidence (green/amber/red); re-click to fix\n\
                     Flags      P pick · X reject · U clear · 1–5/0 stars\n\
                     Burst ops  Ctrl+Enter accept top-2 + reject rest · Ctrl+X reject all\n\
                     Other      O sort time/sharpness · Ctrl+Z undo · Ctrl+H harvest\n\
                     Help       ? or H toggles this window",
                );
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let ctx = ctx.clone();
        self.frame_no += 1;
        self.drain_events(&ctx);
        if !self.harvest.open {
            self.handle_keys(&ctx);
        }

        egui::TopBottomPanel::top("header").show(&ctx, |ui| self.draw_header(ui));
        egui::CentralPanel::default().show(&ctx, |ui| {
            if !self.scan_done {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            }
            match self.view {
                View::Overview => self.draw_overview(ui),
                View::Burst { b, frame } => self.draw_burst_view(ui, b, frame),
            }
        });
        self.draw_harvest(&ctx);
        self.draw_help(&ctx);

        // keep streaming while background work exists
        if self.engine.take_dirty() {
            ctx.request_repaint();
        } else if self.engine.pending() > 0 || !self.scan_done {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }

        if self.session_dirty && self.last_save.elapsed().as_secs_f32() > 2.0 {
            self.save_session();
        }

        // screenshot self-test mode
        if let Some(path) = self.screenshot.clone() {
            if self.frame_no == self.shot_frames {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            let shot = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                let [w, h] = img.size;
                let rgba: Vec<u8> = img
                    .pixels
                    .iter()
                    .flat_map(|p| [p.r(), p.g(), p.b(), p.a()])
                    .collect();
                let _ = image::save_buffer(
                    &path,
                    &rgba,
                    w as u32,
                    h as u32,
                    image::ColorType::Rgba8,
                );
                std::process::exit(0);
            }
            ctx.request_repaint();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.session_dirty {
            self.save_session();
        }
    }
}
