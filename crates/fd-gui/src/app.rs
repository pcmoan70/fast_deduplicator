use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Key, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};
use fd_core::burst::{self, Burst, LogicalImage};
use fd_core::formats::Source;
use fd_core::meta::FileMeta;
use fd_core::pipeline::{Engine, Event, EyeMark, JobKind, RoiKind, TrackRequest};
use fd_core::track::CONF_OK;
use serde::{Deserialize, Serialize};
use fd_core::recipe::{self, Issue, Op, PlannedAction, Recipe, Report, Severity, RECIPE_VERSION};

const AUTO_PICK_N: usize = 2;
const THUMB_BUDGET: usize = 1500;
const PREVIEW_BUDGET: usize = 8;
// Current frame + both inspect-mode neighbors (a 45 MP RGBA texture is
// ~180 MB, so this budget is deliberately tight).
const FULL_BUDGET: usize = 3;
/// Upper bound for wheel zoom, relative to native pixels.
const MAX_ZOOM: f32 = 8.0;
/// Default focus measuring area: half-size as a fraction of the long edge
/// (96 px on a 1620 px preview, the value the ROI score always used).
const DEFAULT_ROI_FRAC: f32 = 96.0 / 1620.0;

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
    /// Fraction of the focus area passing the peaking test.
    Coverage,
}

/// Everything the user can do, from any input route. The keyboard map, the
/// menu bar and the toolbar all emit these; `App::perform` is the only place
/// that acts on them, so the three routes can never drift apart.
#[derive(Clone, Copy, PartialEq)]
enum Action {
    // File
    OpenFolder,
    OpenRecipe,
    Harvest,
    SaveSession,
    Quit,
    // Edit
    Undo,
    Pick,
    Reject,
    ClearFlag,
    Rate(u8),
    RateUp,
    RateDown,
    // Navigation
    OpenBurst,
    BackToOverview,
    NextUnculled,
    NavLeft,
    NavRight,
    NavUp,
    NavDown,
    // Burst operations
    AcceptTop,
    RejectAll,
    ClearTrack,
    ToggleAfPins,
    Unpin,
    // View
    ToggleZoom,
    ToggleInspect,
    ToggleSort,
    SetSort(SortMode),
    SetSource(Source),
    ToggleBrighten,
    TogglePeaking,
    // Help
    ToggleHelp,
    ShowAbout,
}

impl Action {
    const ALL: &'static [Action] = &[
        Action::OpenFolder,
        Action::OpenRecipe,
        Action::Harvest,
        Action::SaveSession,
        Action::Quit,
        Action::Undo,
        Action::Pick,
        Action::Reject,
        Action::ClearFlag,
        Action::Rate(0),
        Action::Rate(1),
        Action::Rate(2),
        Action::Rate(3),
        Action::Rate(4),
        Action::Rate(5),
        Action::RateUp,
        Action::RateDown,
        Action::OpenBurst,
        Action::BackToOverview,
        Action::NextUnculled,
        Action::NavLeft,
        Action::NavRight,
        Action::NavUp,
        Action::NavDown,
        Action::AcceptTop,
        Action::RejectAll,
        Action::ClearTrack,
        Action::ToggleAfPins,
        Action::Unpin,
        Action::ToggleZoom,
        Action::ToggleInspect,
        Action::ToggleSort,
        Action::SetSort(SortMode::Time),
        Action::SetSort(SortMode::Sharpness),
        Action::SetSort(SortMode::Coverage),
        Action::SetSource(Source::Embedded),
        Action::SetSource(Source::Full),
        Action::ToggleBrighten,
        Action::TogglePeaking,
        Action::ToggleHelp,
        Action::ShowAbout,
    ];

    fn label(self) -> String {
        match self {
            Action::OpenFolder => "Open Folder…".into(),
            Action::OpenRecipe => "Open Recipe…".into(),
            Action::Harvest => "Harvest…".into(),
            Action::SaveSession => "Save Session".into(),
            Action::Quit => "Quit".into(),
            Action::Undo => "Undo".into(),
            Action::Pick => "Pick".into(),
            Action::Reject => "Reject".into(),
            Action::ClearFlag => "Clear Flag".into(),
            Action::Rate(0) => "No rating".into(),
            Action::Rate(r) => "★".repeat(r as usize),
            Action::RateUp => "One More Star".into(),
            Action::RateDown => "One Star Less".into(),
            Action::OpenBurst => "Open Burst".into(),
            Action::BackToOverview => "Back to Overview".into(),
            Action::NextUnculled => "Next Unculled Burst".into(),
            Action::NavLeft => "Previous Frame".into(),
            Action::NavRight => "Next Frame".into(),
            Action::NavUp => "Previous Burst".into(),
            Action::NavDown => "Next Burst".into(),
            Action::AcceptTop => format!("Accept Top {AUTO_PICK_N} + Reject Rest"),
            Action::RejectAll => "Reject All Frames".into(),
            Action::ClearTrack => "Clear Pins".into(),
            Action::ToggleAfPins => "Use Camera Eye Points".into(),
            Action::Unpin => "Unpin This Frame".into(),
            Action::ToggleZoom => "Zoom 100%".into(),
            Action::ToggleInspect => "Inspect Focus Point".into(),
            Action::ToggleSort => "Toggle Sort Order".into(),
            Action::SetSort(SortMode::Time) => "Sort by Capture Time".into(),
            Action::SetSort(SortMode::Sharpness) => "Sort by Sharpness".into(),
            Action::SetSort(SortMode::Coverage) => "Sort by Focus Coverage".into(),
            Action::SetSource(Source::Embedded) => "Source: Embedded Preview".into(),
            Action::SetSource(Source::Full) => "Source: Full Image".into(),
            Action::ToggleBrighten => "Auto-brighten Dark Images".into(),
            Action::TogglePeaking => "Focus Peaking".into(),
            Action::ToggleHelp => "Keyboard Shortcuts".into(),
            Action::ShowAbout => "About".into(),
        }
    }

    /// Shortcut text shown next to menu items. Empty means mouse-only.
    fn shortcut(self) -> &'static str {
        match self {
            Action::OpenFolder => "Ctrl+O",
            Action::Harvest => "Ctrl+H",
            Action::SaveSession => "Ctrl+S",
            Action::Quit => "Ctrl+Q",
            Action::Undo => "Ctrl+Z",
            Action::Pick => "P",
            Action::Reject => "X",
            Action::ClearFlag => "U",
            Action::Rate(0) => "0",
            Action::Rate(1) => "1",
            Action::Rate(2) => "2",
            Action::Rate(3) => "3",
            Action::Rate(4) => "4",
            Action::Rate(5) => "5",
            Action::RateUp => "+",
            Action::RateDown => "-",
            Action::OpenBurst => "Enter",
            Action::BackToOverview => "Esc",
            Action::NextUnculled => "N",
            Action::NavLeft => "Left",
            Action::NavRight => "Right",
            Action::NavUp => "Up / Shift+Left",
            Action::NavDown => "Down / Shift+Right",
            Action::AcceptTop => "Ctrl+Enter",
            Action::RejectAll => "Ctrl+X",
            Action::ToggleZoom => "Z",
            Action::ToggleInspect => "I",
            Action::ToggleBrighten => "B",
            Action::TogglePeaking => "K",
            Action::Unpin => "Backspace",
            Action::ToggleSort | Action::SetSort(_) => "O",
            Action::ToggleHelp => "?",
            Action::OpenRecipe
            | Action::ClearTrack
            | Action::ShowAbout
            | Action::Rate(_)
            | Action::SetSource(_)
            | Action::ToggleAfPins => "",
        }
    }
}

/// Modifier a binding needs. `None` bindings ignore Shift (so `?` still
/// works on layouts where it is Shift+/) unless the same key also has a
/// Shift binding.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mods {
    None,
    Ctrl,
    Shift,
}

/// The keyboard bindings, as data so they can be checked for collisions.
/// Context-sensitive meaning (an arrow moves the grid cursor in Overview and
/// the frame in Burst) lives in `perform`, not here.
const KEYMAP: &[(Key, Mods, Action)] = &[
    (Key::O, Mods::Ctrl, Action::OpenFolder),
    (Key::H, Mods::Ctrl, Action::Harvest),
    (Key::S, Mods::Ctrl, Action::SaveSession),
    (Key::Q, Mods::Ctrl, Action::Quit),
    (Key::Z, Mods::Ctrl, Action::Undo),
    (Key::X, Mods::Ctrl, Action::RejectAll),
    (Key::Enter, Mods::Ctrl, Action::AcceptTop),
    (Key::Enter, Mods::None, Action::OpenBurst),
    (Key::Escape, Mods::None, Action::BackToOverview),
    (Key::N, Mods::None, Action::NextUnculled),
    (Key::P, Mods::None, Action::Pick),
    (Key::X, Mods::None, Action::Reject),
    (Key::U, Mods::None, Action::ClearFlag),
    (Key::Z, Mods::None, Action::ToggleZoom),
    (Key::I, Mods::None, Action::ToggleInspect),
    (Key::B, Mods::None, Action::ToggleBrighten),
    (Key::K, Mods::None, Action::TogglePeaking),
    (Key::O, Mods::None, Action::ToggleSort),
    (Key::H, Mods::None, Action::ToggleHelp),
    (Key::Questionmark, Mods::None, Action::ToggleHelp),
    (Key::ArrowLeft, Mods::None, Action::NavLeft),
    (Key::ArrowRight, Mods::None, Action::NavRight),
    (Key::ArrowUp, Mods::None, Action::NavUp),
    (Key::ArrowDown, Mods::None, Action::NavDown),
    (Key::Backspace, Mods::None, Action::Unpin),
    (Key::ArrowLeft, Mods::Shift, Action::NavUp),
    (Key::ArrowRight, Mods::Shift, Action::NavDown),
    (Key::Num0, Mods::None, Action::Rate(0)),
    (Key::Num1, Mods::None, Action::Rate(1)),
    (Key::Num2, Mods::None, Action::Rate(2)),
    (Key::Num3, Mods::None, Action::Rate(3)),
    (Key::Num4, Mods::None, Action::Rate(4)),
    (Key::Num5, Mods::None, Action::Rate(5)),
    (Key::Plus, Mods::None, Action::RateUp),
    (Key::Minus, Mods::None, Action::RateDown),
];

/// A pending action that needs confirmation before it runs.
#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    RejectAll { b: usize },
}

struct Tex {
    handle: TextureHandle,
    last_used: u64,
}

/// Click-and-track state for one burst.
#[derive(Default)]
struct RoiTrack {
    req_id: u64,
    /// The user's pins, (meta idx, x, y): every frame follows its nearest
    /// pin. Kept so the track can be re-run when the measuring area changes.
    seeds: Vec<(usize, f32, f32)>,
    /// meta idx -> (x, y, confidence), normalized coordinates.
    points: HashMap<usize, (f32, f32, f32)>,
    /// meta idx -> sharpness at the tracked point (working-res patch).
    roi_scores: HashMap<usize, f32>,
    /// meta idx -> native-res pupil measurement; `points` then holds the
    /// pupil centre.
    eyes: HashMap<usize, (RoiKind, EyeMark)>,
    /// meta idx -> fraction of the focus area passing the peaking test.
    coverage: HashMap<usize, f32>,
    /// Every frame measured: rank by eye widths from now on.
    done: bool,
}

/// Which rows the review table shows.
#[derive(Clone, Copy, PartialEq)]
enum RowFilter {
    All,
    Actions,
    Skips,
    Problems,
}

impl RowFilter {
    fn label(self) -> &'static str {
        match self {
            RowFilter::All => "all rows",
            RowFilter::Actions => "actions only",
            RowFilter::Skips => "skips only",
            RowFilter::Problems => "problems only",
        }
    }
}

/// Harvest is two steps: build a recipe, review it, then execute it.
enum HarvestPhase {
    /// Choose what the recipe should contain.
    Configure,
    /// Read the planned edits and the evidence behind each one.
    Review {
        recipe: Recipe,
        issues: Vec<Issue>,
        filter: RowFilter,
        saved_to: Option<String>,
    },
    Running {
        done: usize,
        total: usize,
    },
    Done(Report),
}

struct HarvestUi {
    open: bool,
    /// XMP rating sidecar next to each copy (the default place for ratings).
    xmp_with_copies: bool,
    /// XMP sidecars next to the originals: opt-in, it touches the source folder.
    xmp_in_place: bool,
    do_copy: bool,
    dest: String,
    phase: HarvestPhase,
    progress: Option<mpsc::Receiver<HarvestMsg>>,
    /// Transient message (save confirmation, load error).
    status: String,
}

impl HarvestUi {
    /// Defaults: copy picks to a `<folder>_keepers` sibling (never inside the
    /// card folder, which would be rescanned as images), ratings next to the
    /// copies, nothing written next to the originals.
    fn for_dir(dir: &std::path::Path) -> HarvestUi {
        let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "photos".into());
        let dest = dir.parent().unwrap_or(dir).join(format!("{name}_keepers"));
        HarvestUi {
            open: false,
            xmp_with_copies: true,
            xmp_in_place: false,
            do_copy: true,
            dest: dest.display().to_string(),
            phase: HarvestPhase::Configure,
            progress: None,
            status: String::new(),
        }
    }
}

/// Non-destructive record of the culling session, `fd-session.json` in the
/// image folder: flags, ratings and manual focus pins by file name. It is the
/// only thing the app ever writes into that folder.
#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
struct SessionFile {
    version: u32,
    #[serde(default)]
    images: BTreeMap<String, SessionImage>,
    /// Manual focus pins: file -> normalized (x, y) in the upright image.
    #[serde(default)]
    pins: BTreeMap<String, (f32, f32)>,
}

#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
struct SessionImage {
    /// "pick", "reject" or absent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    flag: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    rating: u8,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

enum HarvestMsg {
    Progress(usize, usize),
    Done(Box<Report>),
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
    /// What previews, scores and tracks are computed from.
    source: Source,
    /// Magnification relative to native pixels; None = fit to window.
    zoom: Option<f32>,
    /// Focus measuring area half-size, fraction of the long edge (Shift+wheel).
    roi_frac: f32,
    /// Display-only auto brighten (View menu, key B).
    brighten: bool,
    /// Display-only focus peaking overlay (View menu, key K).
    peaking: bool,
    /// Use the camera's eye-AF frames as focus points (Burst menu).
    use_af: bool,
    /// Pins read from the session file, applied when their burst is opened.
    pending_pins: HashMap<usize, (f32, f32)>,
    /// Contact-sheet thumbnail size, 1.0 = 160 px cells (Shift+wheel).
    thumb_scale: f32,
    /// Requested UI zoom (--ui-zoom); None = follow the monitor.
    ui_zoom: Option<f32>,
    /// Last zoom this app applied, to notice a manual Ctrl+Plus/Minus.
    auto_zoom: Option<f32>,
    /// (last estimated monitor height in px, frames it has held steady).
    zoom_est: (f32, u32),
    /// Inspection mode: full-res JPEG at 1:1, auto-centered on the tracked
    /// focus point of each frame (image center when no track exists).
    inspect: bool,
    pan: Vec2,
    textures: HashMap<(JobKind, usize), Tex>,
    requested: HashSet<(JobKind, usize)>,
    frame_no: u64,
    scan_done: bool,
    scan_started: Instant,
    scores_expected: usize,
    harvest: HarvestUi,
    show_help: bool,
    show_about: bool,
    confirm: Option<Confirm>,
    session_dirty: bool,
    last_save: Instant,
    // screenshot self-test
    screenshot: Option<PathBuf>,
    shot_frames: u64,
    open_burst: Option<usize>,
    auto_track: Option<(f32, f32)>,
    /// Self-test: once scoring settles, accept the top N in every burst,
    /// write the resulting recipe here and exit.
    build_recipe_to: Option<PathBuf>,
    /// Self-test: same, but open the Harvest review table instead of exiting.
    open_harvest: bool,
    /// Self-test: enter inspect mode once the burst is open.
    start_inspect: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        dir: PathBuf,
        screenshot: Option<PathBuf>,
        shot_frames: u64,
        open_burst: Option<usize>,
        auto_track: Option<(f32, f32)>,
        build_recipe_to: Option<PathBuf>,
        open_harvest: bool,
        start_inspect: bool,
        source: Source,
        brighten: bool,
        peaking: bool,
        ui_zoom: Option<f32>,
    ) -> Self {
        // Larger base type and roomier buttons than egui's defaults; the
        // screen-derived zoom in `update` scales on top of this.
        cc.egui_ctx.style_mut(|s| {
            for font in s.text_styles.values_mut() {
                font.size *= 1.15;
            }
            s.spacing.button_padding = Vec2::new(8.0, 4.0);
            s.spacing.item_spacing.x = 8.0;
        });
        let workers = std::thread::available_parallelism()
            .map(|v| v.get().saturating_sub(2).max(2))
            .unwrap_or(4);
        let engine = Engine::start(dir.clone(), dir.join(".fd-cache.db"), workers, source);
        engine.set_brighten(brighten);
        engine.set_peaking(peaking);
        let harvest = HarvestUi::for_dir(&dir);
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
            source,
            zoom: None,
            roi_frac: DEFAULT_ROI_FRAC,
            brighten,
            peaking,
            use_af: true,
            pending_pins: HashMap::new(),
            thumb_scale: 1.0,
            ui_zoom,
            auto_zoom: None,
            zoom_est: (0.0, 0),
            inspect: false,
            pan: Vec2::ZERO,
            textures: HashMap::new(),
            requested: HashSet::new(),
            frame_no: 0,
            scan_done: false,
            scan_started: Instant::now(),
            scores_expected: 0,
            harvest,
            show_help: false,
            show_about: false,
            confirm: None,
            session_dirty: false,
            last_save: Instant::now(),
            screenshot,
            shot_frames,
            open_burst,
            auto_track,
            build_recipe_to,
            open_harvest,
            start_inspect,
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
                            if self.start_inspect {
                                self.inspect = true;
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
                    coverage,
                } => {
                    if let Some(&b) = self.track_burst.get(&req_id) {
                        let entry = self.roi.entry(b).or_default();
                        if entry.req_id == req_id {
                            entry.points.insert(idx, (x, y, conf));
                            entry.roi_scores.insert(idx, roi_score);
                            entry.coverage.insert(idx, coverage);
                        }
                    }
                }
                Event::EyePoint { req_id, idx, x, y, kind, eye } => {
                    if let Some(&b) = self.track_burst.get(&req_id) {
                        let entry = self.roi.entry(b).or_default();
                        if entry.req_id == req_id {
                            let conf = entry.points.get(&idx).map(|p| p.2).unwrap_or(1.0);
                            entry.points.insert(idx, (x, y, conf));
                            entry.eyes.insert(idx, (kind, eye));
                        }
                    }
                }
                Event::TrackDone { req_id } => {
                    if let Some(&b) = self.track_burst.get(&req_id) {
                        if let Some(t) = self.roi.get_mut(&b) {
                            if t.req_id == req_id {
                                t.done = true;
                            }
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
            if self.sort == SortMode::Coverage {
                if let (Some(&(_, _, conf)), Some(&c)) = (t.points.get(&id), t.coverage.get(&id)) {
                    return (conf >= CONF_OK, if conf >= CONF_OK { c } else { self.scores.get(&id).copied().unwrap_or(0.0) });
                }
            }
            // Once every frame is measured, rank by pupil edge width (lower =
            // sharper); frames without a pupil drop to the lower tier so a
            // width is never sorted against a patch score.
            if t.done {
                return match t.eyes.get(&id) {
                    Some((_, e)) => (true, 100.0 / e.width_px.max(0.1)),
                    None => (false, self.scores.get(&id).copied().unwrap_or(0.0)),
                };
            }
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
        if self.sort != SortMode::Time {
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
        self.dir.join("fd-session.json")
    }

    fn rel_key(&self, id: usize) -> String {
        self.rel_path(id).to_string_lossy().to_string()
    }

    fn load_session(&mut self) {
        let by_rel: HashMap<String, usize> =
            self.images.iter().map(|li| (self.rel_key(li.primary()), li.primary())).collect();
        if let Ok(text) = std::fs::read_to_string(self.session_path()) {
            let Ok(file) = serde_json::from_str::<SessionFile>(&text) else {
                return;
            };
            for (name, img) in file.images {
                if let Some(&id) = by_rel.get(&name) {
                    let flag = match img.flag.as_str() {
                        "pick" => Flag::Picked,
                        "reject" => Flag::Rejected,
                        _ => Flag::Unrated,
                    };
                    if flag != Flag::Unrated || img.rating > 0 {
                        self.state.insert(id, ImgState { flag, rating: img.rating });
                    }
                }
            }
            for (name, pin) in file.pins {
                if let Some(&id) = by_rel.get(&name) {
                    self.pending_pins.insert(id, pin);
                }
            }
            return;
        }
        // Migrate the pre-JSON tab-separated session once (absolute paths).
        let Ok(text) = std::fs::read_to_string(self.dir.join(".fd-session.tsv")) else {
            return;
        };
        let by_path: HashMap<&str, usize> = self
            .images
            .iter()
            .map(|li| (self.metas[li.primary()].path.to_str().unwrap_or(""), li.primary()))
            .collect();
        for line in text.lines() {
            let mut parts = line.split('\t');
            let (Some(path), Some(flag), Some(rating)) = (parts.next(), parts.next(), parts.next()) else {
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
        // Rewritten as fd-session.json on the next autosave.
        self.session_dirty = !self.state.is_empty();
    }

    fn save_session(&mut self) {
        let mut file = SessionFile { version: 1, ..Default::default() };
        for (&id, st) in &self.state {
            if self.metas.get(id).is_none() {
                continue;
            }
            let flag = match st.flag {
                Flag::Picked => "pick",
                Flag::Rejected => "reject",
                Flag::Unrated => "",
            };
            file.images.insert(self.rel_key(id), SessionImage { flag: flag.into(), rating: st.rating });
        }
        for t in self.roi.values() {
            for &(id, x, y) in &t.seeds {
                if self.metas.get(id).is_some() {
                    file.pins.insert(self.rel_key(id), (x, y));
                }
            }
        }
        if let Ok(json) = serde_json::to_string_pretty(&file) {
            let _ = std::fs::write(self.session_path(), json);
        }
        self.session_dirty = false;
        self.last_save = Instant::now();
    }

    // ---------- input ----------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Never steal keystrokes from a focused text field (Harvest destination).
        if ctx.wants_keyboard_input() {
            return;
        }
        let (ctrl, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let has_shift_binding =
            |key: &Key| KEYMAP.iter().any(|(k, m, _)| k == key && *m == Mods::Shift);
        let hits: Vec<Action> = ctx.input(|i| {
            KEYMAP
                .iter()
                .filter(|(key, mods, _)| {
                    i.key_pressed(*key)
                        && match mods {
                            Mods::Ctrl => ctrl,
                            Mods::Shift => shift && !ctrl,
                            Mods::None => !ctrl && !(shift && has_shift_binding(key)),
                        }
                })
                .map(|(_, _, action)| *action)
                .collect()
        });
        for a in hits {
            if self.enabled(a) {
                self.perform(a, ctx);
            }
        }
    }

    /// Meta index of the logical image under the cursor, if a burst is open.
    fn current_id(&self) -> Option<usize> {
        let View::Burst { b, frame } = self.view else {
            return None;
        };
        let order = self.order(b);
        order.get(frame).map(|&fi| self.bursts[b].images[fi].primary())
    }

    /// Move within a burst. Zoom and pan are kept on purpose: every frame of
    /// a burst has the same dimensions, so the same offset shows the same spot,
    /// which is what makes flipping between zoomed frames comparable.
    fn goto_frame(&mut self, b: usize, frame: usize) {
        let nf = self.bursts[b].images.len();
        if nf == 0 {
            return;
        }
        self.view = View::Burst { b, frame: frame.min(nf - 1) };
    }

    /// Flag the current frame; picks and rejects auto-advance, clearing does not.
    fn flag_current(&mut self, flag: Flag) {
        let View::Burst { b, frame } = self.view else {
            return;
        };
        let Some(id) = self.current_id() else {
            return;
        };
        let mut st = self.img_state(id);
        st.flag = flag;
        self.apply(vec![(id, st)]);
        if flag != Flag::Unrated {
            self.goto_frame(b, frame + 1);
        }
    }

    fn reject_all(&mut self, b: usize) {
        let changes: Vec<(usize, ImgState)> = self.bursts[b]
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

    /// Whether an action applies right now. Drives greying-out, so the menus
    /// show what exists even when it is momentarily unavailable.
    fn enabled(&self, a: Action) -> bool {
        let in_burst = matches!(self.view, View::Burst { .. });
        let has_bursts = !self.bursts.is_empty();
        match a {
            Action::OpenFolder
            | Action::OpenRecipe
            | Action::Quit
            | Action::ToggleHelp
            | Action::ShowAbout
            | Action::ToggleSort
            | Action::SetSort(_)
            | Action::SetSource(_)
            | Action::ToggleBrighten
            | Action::TogglePeaking
            | Action::ToggleAfPins => true,
            Action::SaveSession => !self.state.is_empty(),
            Action::Harvest => has_bursts,
            Action::Undo => !self.undo.is_empty(),
            Action::Pick
            | Action::Reject
            | Action::ClearFlag
            | Action::Rate(_)
            | Action::RateUp
            | Action::RateDown => in_burst,
            Action::OpenBurst => has_bursts && !in_burst,
            Action::BackToOverview => in_burst,
            Action::NextUnculled => has_bursts,
            Action::NavLeft | Action::NavRight | Action::NavUp | Action::NavDown => has_bursts,
            Action::AcceptTop
            | Action::RejectAll
            | Action::ToggleZoom
            | Action::ToggleInspect => in_burst,
            Action::ClearTrack => match self.view {
                View::Burst { b, .. } => self.roi.contains_key(&b),
                View::Overview => false,
            },
            Action::Unpin => match (self.view, self.current_id()) {
                (View::Burst { b, .. }, Some(id)) => self.is_pinned(b, id),
                _ => false,
            },
        }
    }

    /// The single place an action takes effect, whatever route emitted it.
    fn perform(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::OpenFolder => {
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title("Open image folder")
                    .set_directory(&self.dir)
                    .pick_folder()
                {
                    self.open_dir(dir);
                }
            }
            Action::OpenRecipe => self.open_recipe_dialog(),
            Action::Harvest => self.harvest.open = true,
            Action::SaveSession => self.save_session(),
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Undo => self.undo_last(),
            Action::Pick => self.flag_current(Flag::Picked),
            Action::Reject => self.flag_current(Flag::Rejected),
            Action::ClearFlag => self.flag_current(Flag::Unrated),
            Action::Rate(r) => {
                if let Some(id) = self.current_id() {
                    let mut st = self.img_state(id);
                    st.rating = r;
                    self.apply(vec![(id, st)]);
                }
            }
            // Step the rating without wrapping: + stays at 5, - stays at 0.
            Action::RateUp | Action::RateDown => {
                if let Some(id) = self.current_id() {
                    let mut st = self.img_state(id);
                    st.rating = if a == Action::RateUp { (st.rating + 1).min(5) } else { st.rating.saturating_sub(1) };
                    self.apply(vec![(id, st)]);
                }
            }
            Action::OpenBurst => {
                if !self.bursts.is_empty() {
                    let b = self.overview_cursor.min(self.bursts.len() - 1);
                    self.enter_burst(b);
                }
            }
            Action::BackToOverview => {
                if let View::Burst { b, .. } = self.view {
                    self.overview_cursor = b;
                    self.view = View::Overview;
                    self.zoom = None;
                    self.inspect = false;
                }
            }
            Action::NextUnculled => {
                let from = match self.view {
                    View::Overview => self.overview_cursor,
                    View::Burst { b, .. } => b,
                };
                if let Some(nb) = self.next_unculled(from) {
                    match self.view {
                        View::Overview => self.overview_cursor = nb,
                        View::Burst { .. } => self.enter_burst(nb),
                    }
                }
            }
            Action::NavLeft => match self.view {
                View::Overview => self.overview_cursor = self.overview_cursor.saturating_sub(1),
                View::Burst { b, frame } => self.goto_frame(b, frame.saturating_sub(1)),
            },
            Action::NavRight => match self.view {
                View::Overview => {
                    let n = self.bursts.len();
                    if n > 0 {
                        self.overview_cursor = (self.overview_cursor + 1).min(n - 1);
                    }
                }
                View::Burst { b, frame } => self.goto_frame(b, frame + 1),
            },
            Action::NavUp => match self.view {
                View::Overview => {
                    let cols = self.overview_cols(ctx);
                    self.overview_cursor = self.overview_cursor.saturating_sub(cols);
                }
                View::Burst { b, .. } => {
                    if b > 0 {
                        self.enter_burst(b - 1);
                    }
                }
            },
            Action::NavDown => match self.view {
                View::Overview => {
                    let cols = self.overview_cols(ctx);
                    let n = self.bursts.len();
                    if n > 0 {
                        self.overview_cursor = (self.overview_cursor + cols).min(n - 1);
                    }
                }
                View::Burst { b, .. } => {
                    if b + 1 < self.bursts.len() {
                        self.enter_burst(b + 1);
                    }
                }
            },
            Action::AcceptTop => {
                if let View::Burst { b, .. } = self.view {
                    self.accept_burst(b);
                    if let Some(nb) = self.next_unculled(b) {
                        self.enter_burst(nb);
                    }
                }
            }
            Action::RejectAll => {
                if let View::Burst { b, .. } = self.view {
                    self.confirm = Some(Confirm::RejectAll { b });
                }
            }
            // Drops the user's pins; the camera's eye points come back on their own.
            Action::ClearTrack => {
                if let View::Burst { b, .. } = self.view {
                    self.roi.remove(&b);
                    self.session_dirty = true;
                    self.auto_track(b);
                }
            }
            Action::Unpin => {
                if let (View::Burst { b, .. }, Some(id)) = (self.view, self.current_id()) {
                    self.unpin(b, id);
                }
            }
            // For bursts where Eye-AF locked onto the wrong thing throughout.
            Action::ToggleAfPins => {
                self.use_af = !self.use_af;
                if let View::Burst { b, .. } = self.view {
                    let seeds = self.roi.get(&b).map(|t| t.seeds.clone()).unwrap_or_default();
                    if seeds.is_empty() && !self.use_af {
                        self.roi.remove(&b);
                    } else {
                        self.run_track(b, seeds);
                    }
                }
            }
            Action::ToggleZoom => {
                self.zoom = if self.zoom.is_some() { None } else { Some(1.0) };
                self.inspect = false;
                self.pan = Vec2::ZERO;
            }
            Action::ToggleInspect => {
                self.inspect = !self.inspect;
                self.zoom = None;
                self.pan = Vec2::ZERO;
            }
            Action::ToggleSort => {
                self.sort = match self.sort {
                    SortMode::Time => SortMode::Sharpness,
                    SortMode::Sharpness => SortMode::Coverage,
                    SortMode::Coverage => SortMode::Time,
                }
            }
            Action::SetSort(m) => self.sort = m,
            // Previews, scores and tracks all derive from the source, so the
            // folder is reopened with a fresh engine (flags are autosaved).
            Action::SetSource(s) => {
                if self.source != s {
                    self.source = s;
                    self.open_dir(self.dir.clone());
                }
            }
            // Display only: re-decode what is on screen with the new setting.
            Action::ToggleBrighten => {
                self.brighten = !self.brighten;
                self.engine.set_brighten(self.brighten);
                self.textures.clear();
                self.requested.clear();
            }
            // Display only: paint the pixels the coverage score counts.
            Action::TogglePeaking => {
                self.peaking = !self.peaking;
                self.engine.set_peaking(self.peaking);
                self.textures.clear();
                self.requested.clear();
            }
            Action::ToggleHelp => self.show_help = !self.show_help,
            Action::ShowAbout => self.show_about = true,
        }
    }

    /// Point the app at a different folder: save any pending session, replace
    /// the engine (its `Drop` shuts the old worker pool down) and clear all
    /// per-folder state.
    fn open_dir(&mut self, dir: PathBuf) {
        if self.session_dirty {
            self.save_session();
        }
        let workers = std::thread::available_parallelism()
            .map(|v| v.get().saturating_sub(2).max(2))
            .unwrap_or(4);
        self.engine = Engine::start(dir.clone(), dir.join(".fd-cache.db"), workers, self.source);
        self.engine.set_brighten(self.brighten);
        self.engine.set_peaking(self.peaking);
        self.dir = dir;
        self.metas.clear();
        self.images.clear();
        self.bursts.clear();
        self.state.clear();
        self.scores.clear();
        self.roi.clear();
        self.track_burst.clear();
        self.undo.clear();
        self.textures.clear();
        self.requested.clear();
        self.view = View::Overview;
        self.overview_cursor = 0;
        self.zoom = None;
        self.inspect = false;
        self.pan = Vec2::ZERO;
        self.scan_done = false;
        self.scan_started = Instant::now();
        self.scores_expected = 0;
        self.session_dirty = false;
        self.pending_pins.clear();
        self.harvest = HarvestUi::for_dir(&self.dir);
    }

    /// Pin the focus point on `seed_id` (replacing an earlier pin on that
    /// frame) and re-run the burst's track from all its pins.
    fn start_track(&mut self, b: usize, seed_id: usize, nx: f32, ny: f32) {
        let mut seeds = self.roi.get(&b).map(|t| t.seeds.clone()).unwrap_or_default();
        seeds.retain(|s| s.0 != seed_id);
        seeds.push((seed_id, nx, ny));
        self.run_track(b, seeds);
    }

    /// (Re)start the burst's track from the given pins.
    fn run_track(&mut self, b: usize, seeds: Vec<(usize, f32, f32)>) {
        self.next_track_id += 1;
        let req_id = self.next_track_id;
        let frames: Vec<usize> = self.bursts[b].images.iter().map(|li| li.primary()).collect();
        self.track_burst.insert(req_id, b);
        self.roi.insert(
            b,
            RoiTrack {
                req_id,
                seeds: seeds.clone(),
                points: HashMap::new(),
                roi_scores: HashMap::new(),
                eyes: HashMap::new(),
                coverage: HashMap::new(),
                done: false,
            },
        );
        self.engine.request_track(TrackRequest {
            req_id,
            frames,
            seeds,
            roi_frac: self.roi_frac,
            use_af: self.use_af,
        });
        self.session_dirty = true;
    }

    /// Cancel the manual override on one frame: it goes back to the camera's
    /// eye point (or to tracking from the remaining pins).
    fn unpin(&mut self, b: usize, id: usize) {
        let mut seeds = self.roi.get(&b).map(|t| t.seeds.clone()).unwrap_or_default();
        seeds.retain(|s| s.0 != id);
        let has_eye = self.bursts[b].images.iter().any(|li| self.metas[li.primary()].af_box_eye().is_some());
        if seeds.is_empty() && !(self.use_af && has_eye) {
            self.roi.remove(&b);
            self.session_dirty = true;
        } else {
            self.run_track(b, seeds);
        }
    }

    /// Start the burst's track from the camera's eye frames alone, if it has
    /// any and no track exists yet.
    fn auto_track(&mut self, b: usize) {
        if self.roi.contains_key(&b) {
            return;
        }
        let ids: Vec<usize> = self.bursts[b].images.iter().map(|li| li.primary()).collect();
        // Pins saved in the session come back first.
        let seeds: Vec<(usize, f32, f32)> =
            ids.iter().filter_map(|&id| self.pending_pins.remove(&id).map(|(x, y)| (id, x, y))).collect();
        let has_eye = ids.iter().any(|&id| self.metas[id].af_box_eye().is_some());
        if !seeds.is_empty() || (self.use_af && has_eye) {
            self.run_track(b, seeds);
        }
    }

    fn is_pinned(&self, b: usize, id: usize) -> bool {
        self.roi
            .get(&b)
            .is_some_and(|t| t.seeds.iter().any(|s| s.0 == id))
    }

    /// Resize the focus measuring area and re-run the burst's track (if any)
    /// so its ROI scores use the new size.
    fn set_roi_frac(&mut self, b: usize, frac: f32) {
        self.roi_frac = frac.clamp(0.01, 0.4);
        if let Some(seeds) = self.roi.get(&b).map(|t| t.seeds.clone()) {
            self.run_track(b, seeds);
        }
    }

    /// Wheel zoom about `cursor`: the image point under it stays put. Zooming
    /// out past the fit scale drops back to fit (inspect keeps its anchor).
    fn wheel_zoom(
        &mut self,
        cursor: egui::Pos2,
        img_rect: Rect,
        main_rect: Rect,
        dims: Vec2,
        anchor: Vec2,
        factor: f32,
    ) {
        let fit = (main_rect.width() / dims.x)
            .min(main_rect.height() / dims.y)
            .min(4.0);
        let cur = img_rect.width() / dims.x;
        let new = (cur * factor).clamp(fit, MAX_ZOOM);
        if new <= fit * 1.001 && !self.inspect {
            self.zoom = None;
            self.pan = Vec2::ZERO;
            return;
        }
        let u = (cursor - img_rect.min) / img_rect.size();
        let size = dims * new;
        self.pan = (cursor - main_rect.center()) - Vec2::new(u.x * size.x, u.y * size.y)
            + Vec2::new(anchor.x * size.x, anchor.y * size.y);
        self.zoom = Some(new);
    }

    /// Open a burst. The zoom level and inspect mode carry over so a zoomed
    /// comparison continues in the next sequence; the pan recenters.
    fn enter_burst(&mut self, b: usize) {
        self.view = View::Burst { b, frame: 0 };
        self.pan = Vec2::ZERO;
        self.auto_track(b);
    }

    fn overview_cols(&self, ctx: &egui::Context) -> usize {
        let w = ctx.screen_rect().width() - 24.0;
        ((w / (176.0 * self.thumb_scale)).floor() as usize).max(1)
    }

    // ---------- drawing ----------

    // ---------- chrome: menu bar, toolbar, status bar ----------

    /// A menu entry wired to the action layer. `mark` highlights radio and
    /// checkbox items so current settings read straight off the menu. State is
    /// shown by `Button::selected` rather than a glyph: egui's embedded fonts
    /// have no checkmark, and a missing glyph renders as an empty box.
    fn menu_item(&self, ui: &mut egui::Ui, a: Action, mark: Option<bool>) -> bool {
        let btn = egui::Button::new(a.label())
            .shortcut_text(a.shortcut())
            .selected(mark.unwrap_or(false));
        let clicked = ui.add_enabled(self.enabled(a), btn).clicked();
        if clicked {
            ui.close_menu();
        }
        clicked
    }

    fn draw_menu_bar(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut fired = None;
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                for a in [Action::OpenFolder, Action::OpenRecipe, Action::Harvest] {
                    if self.menu_item(ui, a, None) {
                        fired = Some(a);
                    }
                }
                ui.separator();
                if self.menu_item(ui, Action::SaveSession, None) {
                    fired = Some(Action::SaveSession);
                }
                ui.separator();
                if self.menu_item(ui, Action::Quit, None) {
                    fired = Some(Action::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                if self.menu_item(ui, Action::Undo, None) {
                    fired = Some(Action::Undo);
                }
                ui.separator();
                for a in [Action::Pick, Action::Reject, Action::ClearFlag] {
                    if self.menu_item(ui, a, None) {
                        fired = Some(a);
                    }
                }
                ui.separator();
                let current = self.current_id().map(|id| self.img_state(id).rating);
                ui.menu_button("Rating", |ui| {
                    for r in 0..=5u8 {
                        let a = Action::Rate(r);
                        if self.menu_item(ui, a, Some(current == Some(r))) {
                            fired = Some(a);
                        }
                    }
                    ui.separator();
                    for a in [Action::RateUp, Action::RateDown] {
                        if self.menu_item(ui, a, None) {
                            fired = Some(a);
                        }
                    }
                });
            });
            ui.menu_button("View", |ui| {
                for a in [Action::BackToOverview, Action::NextUnculled] {
                    if self.menu_item(ui, a, None) {
                        fired = Some(a);
                    }
                }
                ui.separator();
                for m in [SortMode::Time, SortMode::Sharpness, SortMode::Coverage] {
                    let a = Action::SetSort(m);
                    if self.menu_item(ui, a, Some(self.sort == m)) {
                        fired = Some(a);
                    }
                }
                ui.separator();
                for s in [Source::Embedded, Source::Full] {
                    let a = Action::SetSource(s);
                    if self.menu_item(ui, a, Some(self.source == s)) {
                        fired = Some(a);
                    }
                }
                ui.separator();
                if self.menu_item(ui, Action::ToggleZoom, Some(self.zoom.is_some())) {
                    fired = Some(Action::ToggleZoom);
                }
                if self.menu_item(ui, Action::ToggleInspect, Some(self.inspect)) {
                    fired = Some(Action::ToggleInspect);
                }
                ui.separator();
                if self.menu_item(ui, Action::ToggleBrighten, Some(self.brighten)) {
                    fired = Some(Action::ToggleBrighten);
                }
                if self.menu_item(ui, Action::TogglePeaking, Some(self.peaking)) {
                    fired = Some(Action::TogglePeaking);
                }
            });
            ui.menu_button("Burst", |ui| {
                for a in [Action::AcceptTop, Action::RejectAll] {
                    if self.menu_item(ui, a, None) {
                        fired = Some(a);
                    }
                }
                ui.separator();
                if self.menu_item(ui, Action::ClearTrack, None) {
                    fired = Some(Action::ClearTrack);
                }
                if self.menu_item(ui, Action::ToggleAfPins, Some(self.use_af)) {
                    fired = Some(Action::ToggleAfPins);
                }
                if self.menu_item(ui, Action::Unpin, None) {
                    fired = Some(Action::Unpin);
                }
            });
            ui.menu_button("Help", |ui| {
                for a in [Action::ToggleHelp, Action::ShowAbout] {
                    if self.menu_item(ui, a, None) {
                        fired = Some(a);
                    }
                }
            });
        });
        fired
    }

    fn tool_button(&self, ui: &mut egui::Ui, a: Action, text: &str) -> bool {
        let hint = if a.shortcut().is_empty() {
            a.label()
        } else {
            format!("{}  ({})", a.label(), a.shortcut())
        };
        ui.add_enabled(self.enabled(a), egui::Button::new(text))
            .on_hover_text(hint)
            .clicked()
    }

    fn draw_toolbar(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut fired = None;
        ui.horizontal(|ui| {
            match self.view {
                View::Overview => {
                    if self.tool_button(ui, Action::OpenBurst, "Open burst") {
                        fired = Some(Action::OpenBurst);
                    }
                    if self.tool_button(ui, Action::NextUnculled, "Next unculled") {
                        fired = Some(Action::NextUnculled);
                    }
                }
                View::Burst { b, .. } => {
                    if self.tool_button(ui, Action::BackToOverview, "Back") {
                        fired = Some(Action::BackToOverview);
                    }
                    ui.separator();
                    if self.tool_button(ui, Action::Pick, "Pick") {
                        fired = Some(Action::Pick);
                    }
                    if self.tool_button(ui, Action::Reject, "Reject") {
                        fired = Some(Action::Reject);
                    }
                    if self.tool_button(ui, Action::ClearFlag, "Clear") {
                        fired = Some(Action::ClearFlag);
                    }
                    ui.separator();
                    // Star rating: click a star to set, click the lit one to clear.
                    let current = self.current_id().map(|id| self.img_state(id).rating);
                    for r in 1..=5u8 {
                        let lit = current.is_some_and(|c| c >= r);
                        let star = if lit { "★" } else { "☆" };
                        let a = Action::Rate(if current == Some(r) { 0 } else { r });
                        if ui
                            .add_enabled(self.enabled(a), egui::Button::new(star).frame(false))
                            .on_hover_text(format!("Rate {r}  ({r})"))
                            .clicked()
                        {
                            fired = Some(a);
                        }
                    }
                    ui.separator();
                    if self.tool_button(ui, Action::ToggleZoom, "100%") {
                        fired = Some(Action::ToggleZoom);
                    }
                    if ui
                        .add_enabled(
                            self.enabled(Action::ToggleInspect),
                            egui::Button::new("Inspect").selected(self.inspect),
                        )
                        .on_hover_text("Full-res JPEG centered on the focus point  (I)")
                        .clicked()
                    {
                        fired = Some(Action::ToggleInspect);
                    }
                    ui.separator();
                    if self.tool_button(ui, Action::AcceptTop, &format!("Accept top {AUTO_PICK_N}"))
                    {
                        fired = Some(Action::AcceptTop);
                    }
                    if self.tool_button(ui, Action::RejectAll, "Reject all") {
                        fired = Some(Action::RejectAll);
                    }
                    ui.separator();
                    // Tracking state, so click-to-track is never silent.
                    match self.current_id().and_then(|id| {
                        self.roi.get(&b).and_then(|t| t.points.get(&id)).copied()
                    }) {
                        Some((_, _, conf)) => {
                            let col = if conf >= 0.75 {
                                Color32::from_rgb(80, 200, 90)
                            } else if conf >= CONF_OK {
                                Color32::from_rgb(230, 180, 60)
                            } else {
                                Color32::from_rgb(220, 70, 70)
                            };
                            let pinned = self.current_id().is_some_and(|id| self.is_pinned(b, id));
                            let pin = if pinned { " · pinned" } else { "" };
                            let t = self.roi.get(&b);
                            let eye = self.current_id().and_then(|id| t.and_then(|t| t.eyes.get(&id)).copied());
                            // Eyelids clip part of the rim, so sharp eyes reach ~1.5; motion blur is 2+.
                            let motion = |e: &EyeMark| if e.anisotropy > 1.6 { " · motion" } else { "" };
                            let cov = self
                                .current_id()
                                .and_then(|id| t.and_then(|t| t.coverage.get(&id)))
                                .map(|c| format!(" · {:.0}% in focus", 100.0 * c))
                                .unwrap_or_default();
                            let text = match eye {
                                Some((RoiKind::EyeCamera, e)) => format!("Eye (camera) · {:.1} px{cov}{}{pin}", e.width_px, motion(&e)),
                                Some((RoiKind::EyeTracked, e)) => format!("Eye (tracked) {conf:.2} · {:.1} px{cov}{}{pin}", e.width_px, motion(&e)),
                                None if !t.is_some_and(|t| t.done) => format!("Tracking {conf:.2}{cov}{pin} · measuring…"),
                                None => format!("Area {conf:.2}{cov}{pin} (no pupil found)"),
                            };
                            ui.colored_label(col, text);
                            if pinned && self.tool_button(ui, Action::Unpin, "Unpin") {
                                fired = Some(Action::Unpin);
                            }
                            ui.weak(format!("area {:.0}% (Shift+wheel)", 200.0 * self.roi_frac));
                            if self.tool_button(ui, Action::ClearTrack, "Clear pins") {
                                fired = Some(Action::ClearTrack);
                            }
                        }
                        None if self.roi.contains_key(&b) => {
                            if self.engine.busy() > 0 {
                                ui.spinner();
                                ui.weak("tracking…");
                            } else {
                                ui.colored_label(
                                    Color32::from_rgb(220, 70, 70),
                                    "Track lost on this frame",
                                );
                            }
                            if self.tool_button(ui, Action::ClearTrack, "Clear pins") {
                                fired = Some(Action::ClearTrack);
                            }
                        }
                        None => {
                            ui.weak("Tracking: off — click the subject to track it");
                        }
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.tool_button(ui, Action::Harvest, "Harvest…") {
                    fired = Some(Action::Harvest);
                }
                egui::ComboBox::from_id_salt("sort")
                    .selected_text(match self.sort {
                        SortMode::Time => "sort: time",
                        SortMode::Sharpness => "sort: sharpness",
                        SortMode::Coverage => "sort: coverage",
                    })
                    .show_ui(ui, |ui| {
                        for m in [SortMode::Time, SortMode::Sharpness, SortMode::Coverage] {
                            if ui
                                .selectable_label(self.sort == m, Action::SetSort(m).label())
                                .clicked()
                            {
                                fired = Some(Action::SetSort(m));
                            }
                        }
                    });
            });
        });
        fired
    }

    fn draw_status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let folder = self
                .dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| self.dir.display().to_string());

            if !self.scan_done {
                ui.spinner();
                ui.label(format!(
                    "scanning {folder} … {:.1}s",
                    self.scan_started.elapsed().as_secs_f32()
                ));
                return;
            }

            let picks = self.state.values().filter(|s| s.flag == Flag::Picked).count();
            let rejects = self
                .state
                .values()
                .filter(|s| s.flag == Flag::Rejected)
                .count();
            let culled = self.bursts.iter().filter(|b| self.burst_culled(b)).count();
            ui.label(format!(
                "{folder} · {} images · {} bursts · {}/{} culled · {picks} picks · {rejects} rejects",
                self.images.len(),
                self.bursts.len(),
                culled,
                self.bursts.len(),
            ));
            let busy = self.engine.busy();
            if busy > 0 {
                ui.separator();
                ui.spinner();
                ui.label(format!("working… {busy}"));
            }
            if self.source == Source::Full {
                ui.separator();
                ui.label("source: full image");
            }
            if self.brighten {
                ui.separator();
                ui.label("brightened");
            }
            if self.peaking {
                ui.separator();
                ui.label("peaking");
            }

            if self.scores.len() < self.scores_expected {
                ui.separator();
                let f = self.scores.len() as f32 / self.scores_expected.max(1) as f32;
                ui.add(
                    egui::ProgressBar::new(f)
                        .desired_width(120.0)
                        .text(format!("scoring {}/{}", self.scores.len(), self.scores_expected)),
                );
            }

            // Right side: where we are and what is under the cursor.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let View::Burst { b, frame } = self.view {
                    if let Some(id) = self.current_id() {
                        let st = self.img_state(id);
                        let score = match self.roi.get(&b).map(|t| (t.eyes.get(&id), t.roi_scores.get(&id), t.done)) {
                            Some((Some((kind, e)), _, true)) => format!(
                                "eye {:.1} px · {:.0}% in focus ({})",
                                e.width_px,
                                100.0 * self.roi.get(&b).and_then(|t| t.coverage.get(&id)).copied().unwrap_or(0.0),
                                if *kind == RoiKind::EyeCamera { "camera" } else { "tracked" }
                            ),
                            Some((_, Some(rs), _)) => format!("ROI {rs:.1}"),
                            _ => self
                                .scores
                                .get(&id)
                                .map(|s| format!("{s:.1}"))
                                .unwrap_or_else(|| "…".into()),
                        };
                        let flag = match st.flag {
                            Flag::Picked => " · PICK",
                            Flag::Rejected => " · REJECT",
                            Flag::Unrated => "",
                        };
                        ui.label(format!(
                            "burst {}/{} · frame {}/{} · {} · sharp {score}{flag} {}",
                            b + 1,
                            self.bursts.len(),
                            frame + 1,
                            self.bursts[b].images.len(),
                            self.metas[id]
                                .path
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            "★".repeat(st.rating as usize),
                        ));
                    }
                } else if !self.bursts.is_empty() {
                    ui.label(format!(
                        "burst {}/{}",
                        self.overview_cursor + 1,
                        self.bursts.len()
                    ));
                }
            });
        });
    }

    /// Shown when a scan finds nothing, instead of an empty grid.
    fn draw_empty_state(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut fired = None;
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.heading("No supported images found");
            ui.label(self.dir.display().to_string());
            ui.add_space(4.0);
            ui.label("fast_deduplicator reads Canon CR3, CR2 and JPEG files.");
            ui.add_space(12.0);
            if ui.button("Open Folder…").clicked() {
                fired = Some(Action::OpenFolder);
            }
        });
        fired
    }

    fn draw_confirm(&mut self, ctx: &egui::Context) {
        let Some(Confirm::RejectAll { b }) = self.confirm else {
            return;
        };
        let n = self.bursts.get(b).map(|x| x.images.len()).unwrap_or(0);
        let mut decision: Option<bool> = None;
        let resp = egui::Modal::new(egui::Id::new("confirm_reject_all")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Reject all frames?");
            ui.add_space(6.0);
            ui.label(format!(
                "This flags all {n} frames in burst {} as rejected. Ctrl+Z undoes it.",
                b + 1
            ));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
                if ui
                    .add(egui::Button::new("Reject all").fill(Color32::from_rgb(150, 50, 50)))
                    .clicked()
                {
                    decision = Some(true);
                }
            });
        });
        if resp.should_close() {
            decision.get_or_insert(false);
        }
        match decision {
            Some(true) => {
                self.reject_all(b);
                self.confirm = None;
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    fn draw_about(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut open = self.show_about;
        egui::Window::new("About")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.heading("fast_deduplicator");
                ui.label(format!("version {}", env!("CARGO_PKG_VERSION")));
                ui.add_space(6.0);
                ui.label(
                    "Culling tool for Canon EOS cards. Groups bursts, ranks frames\n\
                     by sharpness at a point you click, and harvests the keepers\n\
                     through a reviewable recipe.",
                );
            });
        self.show_about = open;
    }

    fn draw_overview(&mut self, ui: &mut egui::Ui) {
        // Shift+wheel over the sheet resizes the thumbnails (0.5x-2x; the
        // decoded thumbs are ~200 px, so larger would only blur).
        if ui.rect_contains_pointer(ui.max_rect()) {
            let (dx, dy, shift) = ui.input(|i| (i.raw_scroll_delta.x, i.raw_scroll_delta.y, i.modifiers.shift));
            if shift && dx + dy != 0.0 {
                self.thumb_scale = (self.thumb_scale * 1.1f32.powf((dx + dy) / 50.0)).clamp(0.5, 2.0);
            }
        }
        let cols = self.overview_cols(ui.ctx());
        let cell = Vec2::new(168.0, 150.0) * self.thumb_scale;
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

        let img_rect = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(160.0, 110.0) * self.thumb_scale);
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
            let size = self.textures[&(JobKind::Thumb, cover_id)].handle.size_vec2();
            ui.painter().image(
                tid,
                fit_inside(size, img_rect),
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let painter = ui.painter();
        // Frame-count badge on a dark backing so it reads on pale thumbnails.
        let galley = painter.layout_no_wrap(format!("{n}"), FontId::proportional(13.0), Color32::WHITE);
        let text_rect = Align2::RIGHT_TOP.anchor_size(img_rect.right_top() + Vec2::new(-4.0, 4.0), galley.size());
        painter.rect_filled(text_rect.expand2(Vec2::new(4.0, 2.0)), 3.0, Color32::from_black_alpha(190));
        painter.galley(text_rect.min, galley, Color32::WHITE);
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

        let magnified = self.zoom.is_some() || self.inspect;
        let z = self.zoom.unwrap_or(1.0);
        // Displayed (upright) pixel dimensions; the 1:1 view is sized from these
        // so the preview stand-in lands where the full-res texture will.
        let (dw, dh) = self.metas[id].display_dims();
        let dims = Vec2::new(dw.max(1) as f32, dh.max(1) as f32);
        let full_tid = self.textures.get(&(JobKind::Full, id)).map(|t| t.handle.id());
        let tid = if magnified {
            self.tex(JobKind::Full, id, 100);
            let t = self.textures.get_mut(&(JobKind::Full, id)).map(|t| {
                t.last_used = self.frame_no;
                t.handle.id()
            });
            match t {
                Some(tid) => Some(tid),
                None => self.tex(JobKind::Preview, id, 90),
            }
        } else {
            match full_tid {
                Some(t) => Some(t),
                None => self.tex(JobKind::Preview, id, 90),
            }
        };

        // What the 1:1 view centers on: the frame's tracked focus point in
        // inspect mode, the image center otherwise.
        let anchor = if self.inspect {
            self.roi
                .get(&b)
                .and_then(|t| t.points.get(&id))
                .map(|&(x, y, _)| Vec2::new(x, y))
                .unwrap_or(Vec2::new(0.5, 0.5))
        } else {
            Vec2::new(0.5, 0.5)
        };

        let mut display_rect: Option<Rect> = None;
        if let Some(tid) = tid {
            if magnified {
                if main_resp.dragged() {
                    self.pan += main_resp.drag_delta();
                }
                // zoom x native pixels with the anchor at the viewport center + pan
                let size = dims * z;
                let min =
                    main_rect.center() - Vec2::new(anchor.x * size.x, anchor.y * size.y) + self.pan;
                let img_rect = Rect::from_min_size(min.round(), size);
                ui.painter().with_clip_rect(main_rect).image(
                    tid,
                    img_rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                display_rect = Some(img_rect);
                let res = if self.textures.contains_key(&(JobKind::Full, id)) {
                    "FULL"
                } else {
                    "PREVIEW"
                };
                let mode = if self.inspect {
                    if self.roi.get(&b).is_some_and(|t| t.points.contains_key(&id)) {
                        format!("INSPECT focus point {:.0}%", z * 100.0)
                    } else {
                        format!("INSPECT center (no track - click the subject) {:.0}%", z * 100.0)
                    }
                } else {
                    format!("{:.0}%", z * 100.0)
                };
                ui.painter().text(
                    main_rect.left_top() + Vec2::new(8.0, 8.0),
                    Align2::LEFT_TOP,
                    format!("{mode} · {res}"),
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

        // Mouse wheel over the image: zoom about the cursor (a notch is x1.25);
        // with Shift, resize the focus measuring area instead. Shift+wheel
        // arrives as horizontal scroll on some platforms, so read both axes.
        if let (Some(img_rect), Some(cursor)) = (display_rect, main_resp.hover_pos()) {
            let (dx, dy, shift) = ui.input(|i| {
                (i.raw_scroll_delta.x, i.raw_scroll_delta.y, i.modifiers.shift)
            });
            let scroll = if shift { dx + dy } else { dy };
            if scroll != 0.0 {
                let factor = 1.25f32.powf(scroll / 50.0);
                if shift {
                    let frac = self.roi_frac * factor;
                    self.set_roi_frac(b, frac);
                } else {
                    self.wheel_zoom(cursor, img_rect, main_rect, dims, anchor, factor);
                }
            }
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

        // Right-click cancels this frame's manual override.
        if main_resp.secondary_clicked() && self.is_pinned(b, id) {
            self.unpin(b, id);
        }

        // ROI overlay on the main image.
        if let Some((img_rect, &(x, y, conf))) = display_rect
            .zip(self.roi.get(&b).and_then(|t| t.points.get(&id)))
        {
            let center = egui::pos2(
                img_rect.min.x + x * img_rect.width(),
                img_rect.min.y + y * img_rect.height(),
            );
            // The box is the focus measuring area (Shift+wheel resizes it).
            let side = 2.0 * self.roi_frac * img_rect.width().max(img_rect.height());
            let color = if conf >= 0.75 {
                Color32::from_rgb(80, 200, 90)
            } else if conf >= CONF_OK {
                Color32::from_rgb(230, 180, 60)
            } else {
                Color32::from_rgb(220, 70, 70)
            };
            let width = if self.is_pinned(b, id) { 3.5 } else { 2.0 };
            match self.roi.get(&b).and_then(|t| t.eyes.get(&id)) {
                // Measured pupil: the circle is the fitted pupil itself.
                Some((_, eye)) => {
                    let r = eye.r_frac * img_rect.width().max(img_rect.height());
                    ui.painter().with_clip_rect(main_rect).circle_stroke(center, r, Stroke::new(width, color));
                }
                None => {
                    ui.painter().with_clip_rect(main_rect).rect_stroke(
                        Rect::from_center_size(center, Vec2::splat(side)),
                        2.0,
                        Stroke::new(width, color),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }

        // The camera's eye frame is always shown when present: dashed green
        // while it is the focus point in use, dashed orange once a manual pin
        // (or Use Camera Eye Points off) overrides it.
        if let (Some(img_rect), Some(af)) = (display_rect, self.metas[id].af_box_eye()) {
            let c = egui::pos2(img_rect.min.x + af.cx * img_rect.width(), img_rect.min.y + af.cy * img_rect.height());
            let r = Rect::from_center_size(c, Vec2::new(af.w * img_rect.width(), af.h * img_rect.height()));
            let overridden = self.is_pinned(b, id) || !self.use_af;
            let color = if overridden { Color32::from_rgb(255, 140, 0) } else { Color32::from_rgb(80, 200, 90) };
            let painter = ui.painter().with_clip_rect(main_rect);
            for (p, q) in [
                (r.left_top(), r.right_top()),
                (r.right_top(), r.right_bottom()),
                (r.right_bottom(), r.left_bottom()),
                (r.left_bottom(), r.left_top()),
            ] {
                painter.extend(egui::Shape::dashed_line(&[p, q], Stroke::new(1.5, color), 6.0, 4.0));
            }
        }

        // Filename, score and position now live in the status bar; only the
        // flag stays on the photo, where it is the one thing worth glancing at.
        let st = self.img_state(id);
        if st.flag != Flag::Unrated {
            let (text, color) = match st.flag {
                Flag::Picked => ("PICK", Color32::from_rgb(80, 200, 90)),
                _ => ("REJECT", Color32::from_rgb(220, 70, 70)),
            };
            let at = main_rect.left_bottom() + Vec2::new(10.0, -10.0);
            let galley = ui.painter().layout_no_wrap(
                text.to_string(),
                FontId::proportional(15.0),
                Color32::WHITE,
            );
            let bg = Rect::from_min_size(
                at - Vec2::new(0.0, galley.size().y + 8.0),
                galley.size() + Vec2::new(16.0, 8.0),
            );
            ui.painter().rect_filled(bg, 4.0, color.gamma_multiply(0.85));
            ui.painter().galley(
                bg.min + Vec2::new(8.0, 4.0),
                galley,
                Color32::WHITE,
            );
        }

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
                            self.goto_frame(b, pos);
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let img_rect =
                            Rect::from_min_size(rect.min + Vec2::new(2.0, 2.0), Vec2::new(136.0, 92.0));
                        ui.painter().rect_filled(img_rect, 3.0, Color32::from_gray(25));
                        // Where the thumbnail actually lands (letterboxed when portrait).
                        let mut shown = img_rect;
                        if let Some(tid) = self.tex(JobKind::Thumb, fid, 60) {
                            let size = self.textures[&(JobKind::Thumb, fid)].handle.size_vec2();
                            shown = fit_inside(size, img_rect);
                            let sst = self.img_state(fid);
                            let tint = if sst.flag == Flag::Rejected {
                                Color32::from_gray(110)
                            } else {
                                Color32::WHITE
                            };
                            ui.painter().image(
                                tid,
                                shown,
                                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                tint,
                            );
                        }
                        // mini ROI box with confidence color
                        if let Some(&(x, y, conf)) =
                            self.roi.get(&b).and_then(|t| t.points.get(&fid))
                        {
                            let c = egui::pos2(
                                shown.min.x + x * shown.width(),
                                shown.min.y + y * shown.height(),
                            );
                            let color = if conf >= 0.75 {
                                Color32::from_rgb(80, 200, 90)
                            } else if conf >= CONF_OK {
                                Color32::from_rgb(230, 180, 60)
                            } else {
                                Color32::from_rgb(220, 70, 70)
                            };
                            let side = (2.0 * self.roi_frac * shown.width().max(shown.height())).max(4.0);
                            ui.painter().with_clip_rect(shown).rect_stroke(
                                Rect::from_center_size(c, Vec2::splat(side)),
                                1.0,
                                Stroke::new(if self.is_pinned(b, fid) { 3.0 } else { 1.5 }, color),
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
                        let s = match self.roi.get(&b).map(|t| (t.eyes.get(&fid), t.roi_scores.get(&fid), t.done)) {
                            _ if self.sort == SortMode::Coverage && self.roi.get(&b).and_then(|t| t.coverage.get(&fid)).is_some() => {
                                format!("{:.0}%", 100.0 * self.roi[&b].coverage[&fid])
                            }
                            Some((Some((_, e)), _, true)) => format!("{:.1}px", e.width_px),
                            Some((_, Some(rs), _)) => format!("•{rs:.1}"),
                            _ => self
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
        // In inspect mode flipping frames is the whole point: pre-decode the
        // neighbors' full-res JPEGs so Left/Right lands on FULL, not PREVIEW.
        if self.inspect {
            if frame + 1 < order.len() {
                let nid = self.bursts[b].images[order[frame + 1]].primary();
                self.tex(JobKind::Full, nid, 95);
            }
            if frame >= 1 {
                let nid = self.bursts[b].images[order[frame - 1]].primary();
                self.tex(JobKind::Full, nid, 94);
            }
        }
    }

    // ---------- harvest: build recipe -> review -> execute ----------

    /// Path of a meta index relative to the scanned folder.
    fn rel_path(&self, id: usize) -> PathBuf {
        let p = &self.metas[id].path;
        p.strip_prefix(&self.dir).unwrap_or(p).to_path_buf()
    }

    /// Turn the current culling session into a recipe. Nothing is written.
    ///
    /// Ranking comes from `order()`, so the recipe records exactly the order
    /// the filmstrip showed — including ROI ranking when a track was running.
    fn build_recipe(&self) -> Recipe {
        let dest = (self.harvest.do_copy && !self.harvest.dest.trim().is_empty())
            .then(|| PathBuf::from(self.harvest.dest.trim()));
        let mut actions: Vec<PlannedAction> = Vec::new();

        for b in 0..self.bursts.len() {
            let order = self.order(b);
            let burst_size = self.bursts[b].images.len();
            for (rank0, &fi) in order.iter().enumerate() {
                let li = self.bursts[b].images[fi].clone();
                let id = li.primary();
                let st = self.img_state(id);
                let track = self.roi.get(&b);
                let conf = track.and_then(|t| t.points.get(&id)).map(|&(_, _, c)| c);
                let roi_s = track.and_then(|t| t.roi_scores.get(&id)).copied();
                let sharp = self.scores.get(&id).copied().unwrap_or(0.0);
                let rank = rank0 + 1;

                let eye_kind = track.filter(|t| t.done).and_then(|t| t.eyes.get(&id)).map(|(k, _)| *k);
                let basis = match eye_kind {
                    Some(RoiKind::EyeCamera) => "eye edge acuity (camera eye point)",
                    Some(RoiKind::EyeTracked) => "eye edge acuity (tracked eye)",
                    None => match (roi_s, conf) {
                        (Some(_), Some(c)) if c >= CONF_OK => "sharpness at the tracked point",
                        (_, Some(_)) => "overall sharpness (track lost here)",
                        _ => "overall sharpness",
                    },
                };
                let evidence = |verb: &str| {
                    format!("{verb}: rank {rank}/{burst_size} in burst {} by {basis}", b + 1)
                };

                let mk = |op: Op, reason: String| PlannedAction {
                    file: self.rel_path(id),
                    op,
                    enabled: true,
                    burst: b + 1,
                    burst_size,
                    rank,
                    sharpness: sharp,
                    roi_sharpness: roi_s,
                    track_confidence: conf,
                    reason,
                };

                if st.flag == Flag::Picked {
                    if self.harvest.xmp_in_place {
                        actions.push(mk(
                            Op::WriteXmp {
                                rating: st.rating.max(3),
                            },
                            evidence("picked"),
                        ));
                    }
                    if let Some(dest) = &dest {
                        let rating = if self.harvest.xmp_with_copies { st.rating.max(3) } else { 0 };
                        actions.push(mk(
                            Op::Copy { dest: dest.clone(), rating },
                            evidence("picked"),
                        ));
                        // A paired JPEG travels with its RAW.
                        if let Some(j) = li.jpeg.filter(|&j| Some(j) != li.raw) {
                            actions.push(PlannedAction {
                                file: self.rel_path(j),
                                op: Op::Copy { dest: dest.clone(), rating: 0 },
                                enabled: true,
                                burst: b + 1,
                                burst_size,
                                rank,
                                sharpness: sharp,
                                roi_sharpness: roi_s,
                                track_confidence: conf,
                                reason: format!("JPEG paired with {}", self.rel_path(id).display()),
                            });
                        }
                    }
                } else {
                    let why = match st.flag {
                        Flag::Rejected => evidence("rejected"),
                        _ => evidence("not picked"),
                    };
                    actions.push(mk(Op::Skip, why));
                }
            }
        }

        Recipe {
            version: RECIPE_VERSION,
            folder: self.dir.clone(),
            created_unix: Recipe::now_unix(),
            settings: recipe::Settings {
                top_n: AUTO_PICK_N,
                gap_secs: 60,
                write_xmp: self.harvest.xmp_in_place,
                copy_to: dest,
            },
            actions,
        }
    }

    fn enter_review(&mut self, r: Recipe) {
        let issues = recipe::check(&r);
        // Write the recipe next to the images so the record outlives the app.
        let path = self.dir.join("cull-recipe.json");
        let saved_to = match r.save(&path) {
            Ok(()) => Some(path.display().to_string()),
            Err(e) => {
                self.harvest.status = format!("could not write {}: {e}", path.display());
                None
            }
        };
        self.harvest.phase = HarvestPhase::Review {
            recipe: r,
            issues,
            filter: RowFilter::All,
            saved_to,
        };
    }

    fn open_recipe_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Open cull recipe")
            .add_filter("Recipe JSON", &["json"])
            .set_directory(&self.dir)
            .pick_file()
        else {
            return;
        };
        match Recipe::load(&path) {
            Ok(r) => {
                let issues = recipe::check(&r);
                self.harvest.open = true;
                self.harvest.status = format!("loaded {}", path.display());
                self.harvest.phase = HarvestPhase::Review {
                    recipe: r,
                    issues,
                    filter: RowFilter::All,
                    saved_to: Some(path.display().to_string()),
                };
            }
            Err(e) => {
                self.harvest.open = true;
                self.harvest.status = format!("could not read {}: {e}", path.display());
                self.harvest.phase = HarvestPhase::Configure;
            }
        }
    }

    fn start_execute(&mut self, r: Recipe) {
        let (tx, rx) = mpsc::channel();
        self.harvest.progress = Some(rx);
        self.harvest.status.clear();
        self.harvest.phase = HarvestPhase::Running {
            done: 0,
            total: r.actions.len(),
        };
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let report = recipe::execute(&r, false, move |done, total| {
                let _ = tx2.send(HarvestMsg::Progress(done, total));
            });
            let _ = tx.send(HarvestMsg::Done(Box::new(report)));
        });
    }

    fn draw_harvest(&mut self, ctx: &egui::Context) {
        if !self.harvest.open {
            return;
        }

        // Drain execution progress before drawing.
        if let Some(rx) = self.harvest.progress.take() {
            let mut finished = false;
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    HarvestMsg::Progress(done, total) => {
                        self.harvest.phase = HarvestPhase::Running { done, total };
                    }
                    HarvestMsg::Done(report) => {
                        self.harvest.phase = HarvestPhase::Done(*report);
                        finished = true;
                    }
                }
            }
            if !finished {
                self.harvest.progress = Some(rx);
            }
        }

        let mut open = self.harvest.open;
        let wide = matches!(self.harvest.phase, HarvestPhase::Review { .. });
        egui::Window::new("Harvest")
            .open(&mut open)
            .collapsible(false)
            .resizable(wide)
            .default_width(if wide { 1100.0 } else { 380.0 })
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| match self.harvest.phase {
                HarvestPhase::Configure => self.draw_harvest_configure(ui),
                HarvestPhase::Review { .. } => self.draw_harvest_review(ui),
                HarvestPhase::Running { .. } => self.draw_harvest_running(ui),
                HarvestPhase::Done(_) => self.draw_harvest_done(ui),
            });
        self.harvest.open = open;
        if !self.harvest.open {
            self.harvest.phase = HarvestPhase::Configure;
        }
    }

    fn draw_harvest_configure(&mut self, ui: &mut egui::Ui) {
        let picks = self
            .images
            .iter()
            .map(|li| li.primary())
            .filter(|id| self.img_state(*id).flag == Flag::Picked)
            .count();

        ui.label(format!("{picks} picked images across {} bursts", self.bursts.len()));
        ui.add_space(4.0);
        ui.label("Step 1 of 2 — build a recipe of the edits. Nothing is written yet.");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.harvest.do_copy, "Copy picks to:");
            ui.text_edit_singleline(&mut self.harvest.dest);
            if ui.button("Browse…").clicked() {
                if let Some(d) = rfd::FileDialog::new()
                    .set_title("Copy picks to")
                    .pick_folder()
                {
                    self.harvest.dest = d.display().to_string();
                    self.harvest.do_copy = true;
                }
            }
        });
        ui.checkbox(&mut self.harvest.xmp_with_copies, "Add XMP rating sidecars next to the copies");
        ui.checkbox(
            &mut self.harvest.xmp_in_place,
            "Also write XMP sidecars next to the originals (touches the source folder)",
        );
        ui.weak("Originals are never moved, changed or deleted; flags, ratings and pins live in fd-session.json.");

        if !self.harvest.status.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(Color32::from_rgb(230, 180, 60), self.harvest.status.clone());
        }

        ui.add_space(8.0);
        let can_build = picks > 0
            && (self.harvest.xmp_in_place || (self.harvest.do_copy && !self.harvest.dest.trim().is_empty()));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_build, egui::Button::new("Build recipe"))
                .clicked()
            {
                let r = self.build_recipe();
                self.enter_review(r);
            }
            if ui.button("Open recipe…").clicked() {
                self.open_recipe_dialog();
            }
        });
        if picks == 0 {
            ui.label("Pick at least one frame first (P in a burst).");
        }
    }

    fn draw_harvest_review(&mut self, ui: &mut egui::Ui) {
        // Move the phase out so the table can mutate rows while `self` stays
        // free for the buttons below; it is put back before returning.
        let mut phase = std::mem::replace(&mut self.harvest.phase, HarvestPhase::Configure);
        let HarvestPhase::Review {
            recipe: r,
            issues,
            filter,
            saved_to,
        } = &mut phase
        else {
            self.harvest.phase = phase;
            return;
        };

        let by_action: HashMap<usize, &Issue> =
            issues.iter().map(|i| (i.action, i)).collect();
        let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
        let warnings = issues.len() - errors;

        ui.label("Step 2 of 2 — review what will happen, then execute.");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} actions · {} rows · burst grouping and ranking as shown in the filmstrip",
                r.active_count(),
                r.actions.len()
            ));
            if errors > 0 {
                ui.colored_label(Color32::from_rgb(220, 70, 70), format!("{errors} errors"));
            }
            if warnings > 0 {
                ui.colored_label(
                    Color32::from_rgb(230, 180, 60),
                    format!("{warnings} warnings"),
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label("Show:");
            for f in [
                RowFilter::All,
                RowFilter::Actions,
                RowFilter::Skips,
                RowFilter::Problems,
            ] {
                if ui.selectable_label(*filter == f, f.label()).clicked() {
                    *filter = f;
                }
            }
        });
        ui.add_space(4.0);

        let active_filter = *filter;
        let show = |i: usize, a: &PlannedAction| -> bool {
            match active_filter {
                RowFilter::All => true,
                RowFilter::Actions => !matches!(a.op, Op::Skip),
                RowFilter::Skips => matches!(a.op, Op::Skip),
                RowFilter::Problems => by_action.contains_key(&i),
            }
        };

        egui::ScrollArea::vertical()
            .max_height(460.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("recipe_rows")
                    .striped(true)
                    .num_columns(8)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("run");
                        ui.label("file");
                        ui.label("operation");
                        ui.label("burst");
                        ui.label("rank");
                        ui.label("sharp");
                        ui.label("track");
                        ui.label("why / status");
                        ui.end_row();

                        for (i, a) in r.actions.iter_mut().enumerate() {
                            if !show(i, a) {
                                continue;
                            }
                            let skip = matches!(a.op, Op::Skip);
                            ui.add_enabled(!skip, egui::Checkbox::without_text(&mut a.enabled));
                            ui.label(a.file.display().to_string());
                            ui.label(a.op_label());
                            ui.label(format!("{}", a.burst));
                            ui.label(format!("{}/{}", a.rank, a.burst_size));
                            ui.label(match a.roi_sharpness {
                                Some(rs) => format!("{rs:.1} roi"),
                                None => format!("{:.1}", a.sharpness),
                            });
                            match a.track_confidence {
                                Some(c) => {
                                    let col = if c >= 0.75 {
                                        Color32::from_rgb(80, 200, 90)
                                    } else if c >= CONF_OK {
                                        Color32::from_rgb(230, 180, 60)
                                    } else {
                                        Color32::from_rgb(220, 70, 70)
                                    };
                                    ui.colored_label(col, format!("{c:.2}"));
                                }
                                None => {
                                    ui.label("—");
                                }
                            }
                            // The evidence stays visible even when a row also
                            // has a warning: both are the point of the review.
                            ui.horizontal(|ui| {
                                ui.label(a.reason.clone());
                                if let Some(issue) = by_action.get(&i) {
                                    let col = if issue.severity == Severity::Error {
                                        Color32::from_rgb(220, 70, 70)
                                    } else {
                                        Color32::from_rgb(230, 180, 60)
                                    };
                                    ui.colored_label(col, format!("· {}", issue.message));
                                }
                            });
                            ui.end_row();
                        }
                    });
            });

        ui.add_space(6.0);
        if let Some(p) = saved_to {
            ui.label(format!("recipe saved to {p}"));
        }

        let recipe_snapshot = r.clone();
        let active = recipe_snapshot.active_count();
        let mut go_back = false;
        let mut save_as = false;
        let mut execute = false;
        ui.horizontal(|ui| {
            go_back = ui.button("Back").clicked();
            save_as = ui.button("Save recipe as…").clicked();
            execute = ui
                .add_enabled(
                    active > 0,
                    egui::Button::new(format!("Execute {active} actions")),
                )
                .on_hover_text("Runs only the ticked rows")
                .clicked();
        });
        if !self.harvest.status.is_empty() {
            ui.label(self.harvest.status.clone());
        }

        // Put the (possibly edited) phase back before acting on the buttons.
        self.harvest.phase = phase;

        if save_as {
            if let Some(p) = rfd::FileDialog::new()
                .set_title("Save cull recipe")
                .set_file_name("cull-recipe.json")
                .add_filter("Recipe JSON", &["json"])
                .save_file()
            {
                self.harvest.status = match recipe_snapshot.save(&p) {
                    Ok(()) => format!("saved {}", p.display()),
                    Err(e) => format!("save failed: {e}"),
                };
            }
        }
        if go_back {
            self.harvest.phase = HarvestPhase::Configure;
        } else if execute {
            self.start_execute(recipe_snapshot);
        }
    }

    fn draw_harvest_running(&mut self, ui: &mut egui::Ui) {
        let HarvestPhase::Running { done, total } = &self.harvest.phase else {
            return;
        };
        let (done, total) = (*done, *total);
        ui.label("Executing recipe…");
        ui.add(
            egui::ProgressBar::new(if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            })
            .text(format!("{done}/{total}")),
        );
    }

    fn draw_harvest_done(&mut self, ui: &mut egui::Ui) {
        let HarvestPhase::Done(report) = &self.harvest.phase else {
            return;
        };
        let summary = report.summary();
        let failures: Vec<String> = report
            .failures()
            .map(|o| format!("{}: {}", o.file.display(), o.error.clone().unwrap_or_default()))
            .collect();

        ui.label(summary);
        if !failures.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(Color32::from_rgb(220, 70, 70), "failed:");
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for f in &failures {
                    ui.label(f);
                }
            });
        }
        ui.add_space(8.0);
        if ui.button("Close").clicked() {
            self.harvest.open = false;
            self.harvest.phase = HarvestPhase::Configure;
        }
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
                    "Overview   arrow keys move · Enter opens a burst\n\
                                 Shift+wheel resizes the thumbnails\n\
                     Burst      Left/Right frame · Shift+Left/Right or Up/Down\n\
                                previous/next burst (zoom kept) · Esc back\n\
                     Flags      P pick · X reject · U clear · 1-5 stars · 0 none\n\
                                +/- one star more/less (stops at 5 and 0)\n\
                     Burst ops  Ctrl+Enter accept top 2 + reject rest\n\
                                Ctrl+X reject all (asks first)\n\
                     View       Z zoom 100% · mouse wheel zooms about the cursor\n\
                                (drag to pan; the spot is kept while you flip\n\
                                frames) · I inspect (full-res JPEG locked\n\
                                onto the tracked focus point; Left/Right flips\n\
                                frames with the eye pinned in place)\n\
                                O sort time / sharpness / focus coverage\n\
                                B auto-brighten dark images (display only)\n\
                                K focus peaking: red = passes the focus test\n\
                                Ctrl+Plus/Minus/0 UI size (auto-set from the screen)\n\
                                View > Source: embedded preview or full image\n\
                                (reopens the folder; flags are kept)\n\
                                N next unculled burst · Ctrl+Z undo\n\
                     Track      the camera's Eye-AF frame places the focus point on\n\
                                every frame it detected (Burst > Use Camera Eye\n\
                                Points); the pupil is measured at full resolution\n\
                                and badges show its edge width in px (lower is\n\
                                sharper). The camera's frame is the dashed box:\n\
                                green while in use, orange when a pin overrides it.\n\
                                Click the subject to pin a point when the camera\n\
                                missed — right-click or Backspace unpins the frame\n\
                                again. The pin is tracked through the burst;\n\
                                click another frame to pin the point there too;\n\
                                frames follow their nearest pin (thick box) and\n\
                                re-rank by sharpness there. Shift+wheel resizes\n\
                                the focus measuring area. The toolbar shows\n\
                                tracking confidence and a button to clear it.\n\
                     Harvest    Ctrl+H — builds a recipe (copy the picks to a\n\
                                folder, XMP ratings next to the copies) that you\n\
                                review row by row before executing it. Originals\n\
                                are never moved, changed or deleted; flags, stars\n\
                                and pins live in fd-session.json in the folder.\n\
                     Files      Ctrl+O open folder · Ctrl+S save session\n\
                                Ctrl+Q quit\n\
                     Help       ? or H toggles this window",
                );
                ui.add_space(6.0);
                ui.label("Every action is also in the menu bar, with its shortcut beside it.");
            });
    }
}

/// Largest rect with `tex`'s aspect ratio centered in `cell`, so portrait
/// thumbnails are letterboxed rather than squashed into the landscape cell.
fn fit_inside(tex: Vec2, cell: Rect) -> Rect {
    let scale = (cell.width() / tex.x.max(1.0)).min(cell.height() / tex.y.max(1.0));
    Rect::from_center_size(cell.center(), tex * scale)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let ctx = ctx.clone();
        self.frame_no += 1;
        // UI scale follows the window's own pixel height (fresh every frame;
        // egui's monitor size and maximized flag lag behind on X11): a
        // 1080 px tall window is 1x, a maximized 4K window 2x, in quarter
        // steps, applied only once the height has held steady for five
        // frames so a resize drag does not make it jump. `--ui-zoom` fixes
        // it; a manual Ctrl+Plus/Minus/0 takes over for the session.
        // Some window managers ignore the maximize hint at creation or move
        // the window to another monitor right after: keep asking during the
        // first seconds until it is maximized, then leave it to the user.
        if self.frame_no % 30 == 1
            && self.scan_started.elapsed().as_secs_f32() < 5.0
            && ctx.input(|i| i.viewport().maximized) != Some(true)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        let est = ctx.input(|i| i.screen_rect().height() * i.pixels_per_point());
        let (last, n) = self.zoom_est;
        self.zoom_est = if (est - last).abs() < 0.02 * last { (last, n + 1) } else { (est, 0) };
        let target = match self.ui_zoom {
            Some(z) => (self.frame_no == 1).then_some(z),
            None => (self.zoom_est.1 >= 5).then(|| ((self.zoom_est.0 / 1080.0) * 4.0).round() / 4.0).map(|z| z.clamp(1.0, 3.0)),
        };
        if let Some(z) = target {
            let current = ctx.zoom_factor();
            let user_changed = self.auto_zoom.is_some_and(|last| (current - last).abs() > 0.01);
            if !user_changed && (z - current).abs() > 0.05 {
                eprintln!("ui zoom {z:.2} (window {:.0} px tall)", self.zoom_est.0);
                ctx.set_zoom_factor(z);
                self.auto_zoom = Some(z);
                self.zoom_est.1 = 0;
            }
        }
        self.drain_events(&ctx);
        if !self.harvest.open && self.confirm.is_none() && !self.show_about {
            self.handle_keys(&ctx);
        }

        // Menus and toolbar emit actions; they are performed after the frame's
        // panels are drawn so a view change never lands mid-layout.
        let mut fired = None;
        egui::TopBottomPanel::top("menubar")
            .show(&ctx, |ui| fired = fired.or(self.draw_menu_bar(ui)));
        egui::TopBottomPanel::top("toolbar")
            .show(&ctx, |ui| fired = fired.or(self.draw_toolbar(ui)));
        egui::TopBottomPanel::bottom("statusbar").show(&ctx, |ui| self.draw_status_bar(ui));
        egui::CentralPanel::default().show(&ctx, |ui| {
            if !self.scan_done {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            }
            if self.bursts.is_empty() {
                fired = fired.or(self.draw_empty_state(ui));
                return;
            }
            match self.view {
                View::Overview => self.draw_overview(ui),
                View::Burst { b, frame } => self.draw_burst_view(ui, b, frame),
            }
        });
        if let Some(a) = fired {
            self.perform(a, &ctx);
        }
        self.draw_harvest(&ctx);
        self.draw_help(&ctx);
        self.draw_about(&ctx);
        self.draw_confirm(&ctx);

        // Keep streaming while background work exists, and say so: the
        // cursor is the busy indicator while anything decodes or tracks.
        let busy = self.engine.busy() > 0 || !self.scan_done;
        if busy {
            ctx.set_cursor_icon(egui::CursorIcon::Progress);
        }
        if self.engine.take_dirty() {
            ctx.request_repaint();
        } else if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }

        if self.session_dirty && self.last_save.elapsed().as_secs_f32() > 2.0 {
            self.save_session();
        }

        // Self-test: auto-cull, then open the Harvest review table so the
        // screenshot mode can capture it.
        if self.open_harvest && self.scan_done && self.scores.len() >= self.scores_expected {
            self.open_harvest = false;
            for b in 0..self.bursts.len() {
                self.accept_burst(b);
            }
            self.harvest.xmp_in_place = true;
            self.harvest.open = true;
            let r = self.build_recipe();
            self.enter_review(r);
        }

        // Recipe self-test: wait for scoring to settle so the ranking is
        // deterministic, auto-accept every burst, then write and exit.
        if let Some(out) = self.build_recipe_to.clone() {
            if self.scan_done && self.scores.len() >= self.scores_expected {
                for b in 0..self.bursts.len() {
                    self.accept_burst(b);
                }
                self.harvest.xmp_in_place = true;
                let r = self.build_recipe();
                match r.save(&out) {
                    Ok(()) => println!(
                        "wrote {} ({} actions, {} enabled)",
                        out.display(),
                        r.actions.len(),
                        r.active_count()
                    ),
                    Err(e) => eprintln!("failed to write {}: {e}", out.display()),
                }
                std::process::exit(0);
            }
            ctx.request_repaint();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every action reaches the user as a menu entry or tooltip, so a blank
    /// label would render an invisible control.
    #[test]
    fn every_action_has_a_label() {
        for a in Action::ALL {
            assert!(!a.label().is_empty(), "action has no label");
        }
    }

    /// Two bindings on the same key+modifier would both fire on one press.
    #[test]
    fn keymap_has_no_duplicate_bindings() {
        let mut seen: Vec<(Key, Mods)> = Vec::new();
        for (key, mods, _) in KEYMAP {
            let combo = (*key, *mods);
            assert!(
                !seen.contains(&combo),
                "duplicate binding for {key:?} ({mods:?})"
            );
            seen.push(combo);
        }
    }

    /// The session file is plain JSON and round-trips exactly.
    #[test]
    fn session_file_round_trips() {
        let mut f = SessionFile { version: 1, ..Default::default() };
        f.images.insert("a.JPG".into(), SessionImage { flag: "pick".into(), rating: 4 });
        f.images.insert("b.JPG".into(), SessionImage { flag: "reject".into(), rating: 0 });
        f.pins.insert("a.JPG".into(), (0.61, 0.34));
        let json = serde_json::to_string_pretty(&f).unwrap();
        assert!(json.contains("\"pick\"") && json.contains("\"rating\": 4"));
        assert_eq!(serde_json::from_str::<SessionFile>(&json).unwrap(), f);
    }

    /// A bound action missing from ALL would escape the label check above.
    #[test]
    fn every_bound_action_is_listed() {
        for (key, _, action) in KEYMAP {
            assert!(
                Action::ALL.contains(action),
                "{key:?} is bound to an action missing from Action::ALL"
            );
        }
    }
}
