# Changes

## 2026-09-08 — Docs pass
- ARCHITECTURE: focus coverage / peaking bullet (one gradient test drives both
  the `N% in focus` number and the red overlay) and a note that auto-brighten
  and peaking are display-only engine flags that invalidate no cache.
- In-app help: the tracking paragraph read out of order after the Shift+wheel
  line was inserted; reflowed.
- Verified `fd-gui --peaking --screenshot` on an 8-frame owl copy: overlay
  drawn, chip shows `Eye (camera) · 5.1 px · 11% in focus`, status bar
  `brightened | peaking`. 44 tests pass.

## 2026-09-05 — Eye sharpness from the camera's eye point, busy cursor, HiDPI, auto-brighten
- **Camera eye points.** The Eye-AF frame Canon writes into every JPG and CR3
  (MakerNote AFInfo2) is parsed, so opening a burst places the focus point on
  the eye of every frame the camera detected, with no click. Zone/body frames
  are ignored by size. Verified on landscape and portrait (orientation 8)
  frames.
- **Pupil edge width replaces the patch score at the eye.** Around the focus
  point the full-resolution JPEG is losslessly cropped, the pupil segmented,
  and 64 rays across its rim give the 10-90% rise width in native px (75th
  percentile; lower = sharper). Badges, chip, status and harvest evidence show
  it (`eye 5.4 px (camera)`); `· motion` appears when the blur is directional.
  Frames without a findable pupil, or whose pupil radius disagrees with the
  burst, keep the patch score and rank below measured frames. On the 16-frame
  owl burst the crispest frames by eye rank first and the softest last; on a
  pale-eyed bird 9 of 12 pupils (r = 12 px) are found.
- **When Eye-AF misses:** the camera's eye frame is always drawn as a dashed
  box, green while in use and orange once overridden; a click pins the frame
  and overrides the camera's box there; right-click, Backspace, the toolbar
  `Unpin` button or Burst > Unpin This Frame cancel that frame's override;
  `Clear Pins` returns the whole burst to the camera's points; *Burst > Use Camera
  Eye Points* turns them off for bursts where the camera tracked the wrong
  subject. Shift+wheel now sets the eye search area for frames without a box.
- **Focus peaking** (`K`, View menu, `--peaking`): display-only overlay that
  paints pixels whose smoothed luma gradient exceeds 8 levels/px red, on
  previews and the 1:1 view. **Focus coverage**: the eye measurement now also
  reports the fraction of the focus area (the camera's eye box, or the search
  area) passing that same test, shown as `N% in focus`; *Sort by Focus
  Coverage* is a third sort mode (`O` cycles through all three).
- Shift+wheel over the contact sheet resizes the thumbnails (0.5x to 2x).
- `+` / `-` step the star rating one at a time, stopping at 5 and 0 (no
  wrap-around); also in Edit > Rating.
- **Busy cursor** and `working… N` in the status bar while anything decodes or
  measures; the tracking chip says `measuring…` until a burst's eyes are done.
- **HiDPI.** The window starts maximized and the UI zoom follows the
  window's pixel height in quarter steps (1080 px = 1x, a maximized 4K
  window = 2x), applied only once the height has held steady so resizes do
  not flicker; egui's monitor size and maximized flag were both found to lag
  on X11 and are not used. Base type is 15% larger and buttons roomier;
  Ctrl+Plus/Minus/0 adjust it; `--ui-zoom Z` fixes it.
- **Auto-brighten** (on by default; `B` / View menu toggles it, `--no-brighten`
  starts without): lifts dark frames for viewing only (linear gain so the
  99.5th percentile reaches 235, capped at 4x; bright frames untouched),
  applied in the decode worker; scores never see it.
- **Non-destructive by construction.** The session is now `fd-session.json`
  in the image folder (flags, stars and manual focus pins by file name; the
  old hidden `.fd-session.tsv` is migrated on first save). Harvest defaults
  to copying the picks to a `<folder>_keepers` sibling with an XMP rating
  sidecar next to each copy; sidecars next to the originals are an opt-in
  checkbox. Nothing is ever moved or deleted; the planned trash step is
  dropped. `Op::Copy` carries an optional `rating` (old recipes still load).
- Internals: `formats/canon.rs`, `eye.rs`, `decode::decode_luma_crop`,
  `decode::orient_norm`, `FileMeta::af_box_eye`, `TrackRequest.use_af`,
  `Event::EyePoint`/`TrackDone`, `Engine::busy`, `Engine::set_brighten`;
  `sharpbench --eyes --out DIR` writes pupil crops with the fitted ring so
  segmentation and ranking can be checked by eye. 41 tests.

## 2026-09-04 — Sharper sharpness detector
- The sharpness score is now contrast-normalized Laplacian energy (variance
  of the 5-point Laplacian / luma variance, x100) instead of Sobel energy /
  variance. Chosen with a new benchmark (`cargo run --release -p fd-core
  --example sharpbench -- --point x,y <files>`, `--burst --seed NAME` for a
  real sequence) that degrades real frames at native scale: a 2 px native blur
  is now separated 2.0-3.6x (was 1.13-1.22x) at the same cost, sharp still
  beats blurred under heavy common-mode noise, and the 16-frame owl burst's ROI
  scores spread 3.55x instead of 1.10x, so the badges are no longer all
  "2.7". Eye crops of the top- and bottom-ranked frames agree with the new
  order. The global score gates tiles by contrast so a flat noisy area cannot
  win max-over-tiles.
- Scores are on a new scale (roughly 5-50 instead of 2-12) and cached under a
  versioned key (`score::SCORE_VERSION`), so the first cull re-scores every
  file (~20 s for 5,350 JPGs embedded, ~7 min with `--source full`); old rows
  are orphaned, never mixed. On the test card the top-2 picks changed in 358
  of 596 bursts, mostly bursts where the old scores were within 0.2 of each
  other.
- Measuring the tracked point at native resolution (lossless JPEG crop) was
  benchmarked and not adopted: it did not improve discrimination relative to
  jitter and inflated noise 3-10x while costing a decode per frame. The
  benchmark stays in the repo so the next formula change is measured, not
  guessed.
- New unit tests: fine blur (0.5/1 px at working res) is separated in order
  and by margin, common-mode noise does not invert ranking, exposure flicker
  is invisible, flat noise scores low through `score_global`.

## 2026-09-04 — Focus-point pins, Shift+arrows between bursts
- **Pins.** Clicking the subject on any frame pins the focus point there;
  clicking on another frame adds a second pin instead of restarting. Every
  frame follows its nearest pin (ties go to the newest), so a drifted track is
  fixed by pinning the frame where it went wrong. Pinned frames draw a thicker
  box and the toolbar chip says `pinned`; Clear track removes all pins.
  Internals: `TrackRequest.seeds` replaces the single seed; `run_track` runs
  one wavefront per pin bounded by nearest-pin ownership.
- **Shift+Left/Right** jump to the previous/next burst (same as Up/Down). The
  key table now carries a modifier enum (`Mods::{None, Ctrl, Shift}`) instead
  of a Ctrl flag.
- Opening another burst keeps the zoom level and inspect mode (pan recenters),
  so a zoomed comparison continues in the next sequence.

## 2026-09-04 — Wheel zoom, adjustable focus area, zoom spot kept across frames
- **Mouse wheel zooms** about the cursor (a notch is x1.25, up to 8x native);
  zooming out past fit returns to fit. `Z` still toggles fit / 100%. The chip
  shows the current percentage; inspect mode can be wheel-zoomed too.
- **Shift+wheel resizes the focus measuring area** (the patch the ROI
  sharpness is computed over). It is expressed as a fraction of the long
  edge, so it means the same at preview and full-image resolution; the
  toolbar shows it (`area 12%` by default = the old fixed 192 px patch on a
  1620 px preview) and the burst is re-tracked and re-scored at the new size.
  The green/amber/red box now shows this area rather than the 64 px tracker
  template, on the main image and on the filmstrip.
- In zoom (and inspect), moving to another frame of the same burst by arrow
  key or filmstrip click keeps the current pan offset instead of recentering,
  so every frame is viewed at the same spot. Entering a burst, Esc and
  toggling zoom/inspect still reset it.
- Internals: `TrackRequest.roi_frac`; `App.zoom: Option<f32>` replaces the
  `zoom_100` flag.

## 2026-09-04 — Image source option + upright display
- **Image source.** `View > Source: Embedded Preview | Full Image` (also
  `fd-gui DIR --source full` and `fd cull DIR --source full`). Embedded is the
  previous behaviour (Canon JPG MPF preview, CR3 PRVW). Full uses the JPG
  itself or a CR3's native-res embedded JPEG (CR2 falls back to its IFD0
  preview), decoded at ~2000 px, so sharpness comes from the real pixels
  rather than the camera's re-encoded preview. Governs the main preview,
  scores and eye tracking; thumbnails stay embedded and 100%/inspect was
  already full-size. Full-source scores are cached under their own key
  (`-full` suffix); existing cache entries stay valid. Switching in the GUI
  reopens the folder (flags survive; undo and the current view do not). The
  status bar says `source: full image` while active; `fd cull` prints
  `score[embedded|full]`.
- **Upright display.** EXIF orientation is applied to thumbnails, previews,
  full-res views and the tracking luma, so portrait shots (31% of the test
  card) show portrait and click/track boxes land where you point. Thumbnails
  are letterboxed in their cells instead of stretched. The global score is
  still computed on the stored orientation (the metric is rotation-neutral
  and old cache entries stay valid).
- Internals: `formats::extract_preview` became `extract_jpeg(meta, Source)`;
  `Engine::start` takes the source; the 100% zoom path now goes through the
  same extraction (a CR3 without a JPEG trak previously fell back to decoding
  the raw PRVW container). New tests: `orient` (all 8 EXIF cases + RGBA),
  `display_dims`, cache key forms, and Full-source golden assertions for
  JPG/CR3/CR2.

## 2026-08-03 — Inspect mode
- New `I` shortcut (plus View menu and toolbar button): shows the
  full-resolution embedded JPEG at 1:1, auto-centered on the tracked focus
  point of the current frame. Navigating frames keeps the point pinned at
  the viewport center, so the eye stays put while sharpness varies around
  it. Falls back to image center (labelled in the chip) when no track
  exists. Neighbour frames' full-res JPEGs are prefetched (texture budget
  3) so Left/Right lands on FULL, not PREVIEW. Zoom and inspect are
  mutually exclusive; both drop back to fit on burst change or Esc.

## 2026-08-03 — Menu/toolbar control system + reviewable recipe harvest

**Controls.** The GUI was keyboard-only: three header buttons, everything
else an undocumented shortcut, and the folder a command-line argument.
- Menu bar (File / Edit / View / Burst / Help) covering every action, with
  each shortcut printed beside its entry. Items grey out when they do not
  apply instead of disappearing, so the menus advertise what exists.
- Context-sensitive toolbar per view, every button tooltipped with its
  shortcut; clickable star rating; a tracking indicator with confidence and
  a Clear track button, so click-to-track is no longer silent.
- Status bar: folder, counts, scan/score progress, and cursor position.
  This replaces the text that used to be painted over the photo.
- `File > Open Folder…` (native picker) — the app no longer needs a
  terminal to start. Also `Ctrl+S` save session and `Ctrl+Q` quit.
- `Ctrl+X` (reject whole burst) now asks for confirmation.
- A scan that finds no images shows an explanation and an Open Folder
  button rather than an empty window.
- Internally: one `Action` enum with a single `perform` dispatcher, so
  keyboard, menu and toolbar cannot drift apart. `KEYMAP` is data, and a
  test proves no key+modifier is bound twice.

**Harvest is now two steps.** It used to write sidecars and copy files the
instant you pressed Run, with no way to see what the burst grouping,
ranking and tracker had actually decided.
- New `fd-core/src/recipe.rs`: a `Recipe` is a JSON list of every intended
  edit, each `PlannedAction` carrying its evidence — burst, rank within the
  burst, sharpness, ROI sharpness, tracker confidence and a plain-language
  reason. `Skip` rows are included so what is dropped is reviewable too.
- `recipe::check` is a read-only precondition pass (missing source, sidecar
  overwrite, copy-destination collisions); `recipe::execute` is the single
  writer for GUI and CLI and honours `dry_run` by writing nothing.
- The Harvest dialog is Configure → Review → Running → Done. The review
  table shows every row with its evidence and status, with per-row toggles
  and filters; Execute runs only the ticked rows.
- The recipe is saved to `<folder>/cull-recipe.json`, can be saved
  elsewhere, and reopened later via `File > Open Recipe…`.
- New `fd apply <recipe.json> [--dry-run] [--verbose]` executes a reviewed
  recipe headlessly through the same code path.

**Docs.** README gained per-platform install and run instructions for Linux,
macOS and Windows, including the non-obvious build requirements: CMake and
NASM are needed because libjpeg-turbo is always compiled from vendored
source, and Linux file dialogs go through the XDG desktop portal rather than
GTK. Both escape hatches from the C toolchain are documented
(`TURBOJPEG_SOURCE=pkg-config`, or dropping fd-core's `turbo` feature for the
pure-Rust decoder). Verified by a clean from-scratch build on Linux; the
macOS and Windows steps are derived from dependency requirements, untested.

## 2026-08-01 — Fix: GUI rendered no text
- `fd-gui` drew every label, badge and overlay as nothing: the window
  looked black and the `?` help window was an empty gray box. Cause was
  `default-features = false` on the eframe dependency, which silently
  drops the `default_fonts` feature — egui then starts with an empty font
  set and rasterizes no glyphs, without any error. Re-enabled the feature.
- Added a `default_fonts_are_embedded` test so dropping it again fails
  `cargo test` instead of shipping a blank UI.

## 2026-07-22 — M3 (first cut): click-once eye tracking
- Click any point on the main image (the bird's eye): an NCC template
  tracker follows it bidirectionally through the burst on preview-res luma
  (~45 ms/frame), dual-template drift control, per-frame confidence.
- ROI box overlay on main image and filmstrip thumbs, colored by
  confidence (green/amber/red). Re-click replaces the track; a newer
  request aborts an in-flight one.
- Frames re-rank by sharpness at the tracked point; low-confidence frames
  fall back to global sharpness and always rank below tracked frames.
  Ctrl+Enter accept-burst uses the ROI ranking.
- Unit tests: tracker follows synthetic translations (<1.5 px error),
  reports low confidence on unrelated frames.

## 2026-07-22 — M2 (first cut): GUI
- New `fd-gui` binary (egui/eframe): Card Overview of burst stacks
  (virtualized grid, cull-state rings, pick tallies), Burst View with main
  image + filmstrip (sharpness badges, flag colors), fit/100% zoom with
  progressive PREVIEW→FULL swap and drag pan.
- Keyboard-first culling: arrows/Enter/Esc, N next unculled burst, P/X/U
  flags with auto-advance, 1–5/0 stars, Ctrl+Enter accept top-2 + reject
  rest, Ctrl+X reject burst, O sort time/sharpness, Ctrl+Z undo, ? help.
- Engine in fd-core: scan thread + decode worker pool with priority queue
  (visible frame > neighbors > thumbs > background scoring), SQLite-backed
  score cache shared with the CLI, texture LRU budgets.
- Harvest dialog (Ctrl+H): XMP sidecars + copy picks with live progress.
- Session persistence: flags/ratings autosaved to .fd-session.tsv; resume
  on reopen.
- `--screenshot`/`--open-burst` self-test flags; verified rendering against
  the full 5,540-file corpus copy.

## 2026-07-21 — M1: burst grouping, sharpness ranking, cache, harvest
- RAW+JPEG pairing into logical images; burst grouping by camera serial +
  subsecond timestamp gap (default 0.6 s, `--gap`).
- Sharpness: contrast-normalized Tenengrad on the 1620px embedded preview;
  global score = max over 9×6 tile grid with center bias. (Multi-scale mix
  rejected — per-scale normalization scores blur higher; see tasks/lessons.md.)
- Discovery: Canon EOS JPGs carry a 1620×1080 MPF preview appendix — JPEG
  bursts are scored from ~300 KB instead of a 45 MP decode (40× I/O cut).
- `turbo` cargo feature: libjpeg-turbo (SIMD) with DCT-scaled decode for
  files without an embedded preview; pure-Rust zune-jpeg fallback.
- SQLite score cache keyed by content (xxh3 of first 64 KB + size).
- `fd cull`: auto-pick top-N per burst, `--xmp` sidecars, `--copy-to`
  harvest (sidecars travel along, no overwrites), `--dry-run`; `fd bench`.
- Measured (5,540-file R5 II card): cold cull 13.6 s, warm re-cull 0.17 s,
  595 bursts, 1,124 picks.
- Test-data safety: all runs now target the copy at /home/pc/fd_testdata.

## 2026-07-21 — M0: format core + CLI scan
- Workspace scaffold: `fd-core` (headless library) + `fd-cli` (binary `fd`).
- Bespoke CR3 (ISO-BMFF) header parser: CMT1/CMT2 EXIF, THMB thumbnail,
  CTBO→PRVW preview range, full-size embedded JPEG via trak sample tables.
- Bespoke CR2 (TIFF) parser: IFD0 preview strip, IFD1 thumbnail, EXIF.
- JPEG parser: APP1 EXIF + SOF dimensions.
- Shared endian-aware TIFF/IFD walker; EXIF with subsecond timestamps
  (centisecond precision for burst grouping).
- `fd scan`: parallel header parse, metadata table, `--dump-previews`,
  `--dump-fullsize`, timing stats.
- Golden tests on real R5 Mark II CR3/JPG + CC0 5D Mark III CR2 fixtures.
- Measured: 5,540-file card dump header-scanned in 0.83 s (6,647 files/s,
  1.45 GB I/O instead of 103 GB); previews verified as valid 1620×1080
  JPEGs; full-size embedded JPEGs verified at native 8192×5464.
