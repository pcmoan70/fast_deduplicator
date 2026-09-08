# fast_deduplicator — build plan (started 2026-07-21)

Approved plan: ~/.claude/plans/make-a-plan-for-cheerful-taco.md
Test corpus: /home/pc/temp/EOSR5_20251014/100EOSR5/ (5,575 files, 103 GB, R5 CR3+JPG)

## M0 — Format core + CLI scan  ✅ 2026-07-21
- [x] Workspace scaffold (fd-core, fd-cli), rustup toolchain installed
- [x] TIFF/IFD walker (endian-aware, shared by CR2/EXIF)
- [x] EXIF tag extraction (timestamp+subsec, camera, serial, exposure, lens)
- [x] CR3 parser: ISO-BMFF walker, CMT1/CMT2, THMB, CTBO→PRVW range, trak1 full-size JPEG range
- [x] CR2 parser: IFD0 full-res JPEG range, IFD1 thumb, EXIF
- [x] JPEG parser: APP1 EXIF + SOF dimensions
- [x] fd-cli scan: parallel header parse, metadata table, --dump-previews/--dump-fullsize
- [x] scan prints files/s + MB I/O (dedicated bench subcommand deferred to M1)
- [x] Golden tests on fixtures (R5 II CR3+JPG, 5D3 CR2); EXIF cross-checked vs PIL
- [x] Verify: 6,647 files/s on 5,540-file card dump (target was ≥200); previews valid 1620×1080, fullsize 8192×5464

## M1 — Grouping + scoring + cache + XMP  ✅ 2026-07-21
- [x] RAW+JPEG pairing (logical images) + burst grouping (serial, ts+subsec, gap)
- [x] Sharpness: contrast-normalized Tenengrad, single-scale (multi-scale rejected — see lessons), max-over-tiles global with center bias
- [x] MPF preview discovery in Canon JPGs (1620px appendix; 40x I/O cut for JPEG bursts)
- [x] turbojpeg `turbo` feature (SIMD, DCT-scaled decode) with zune-jpeg fallback; nasm built to ~/.local
- [x] SQLite score cache keyed by (xxh3-64KB, size)
- [x] XMP sidecar writer + copy-picks harvest
- [x] fd cull: top-N per burst, --xmp/--copy-to/--dry-run; fd bench per-stage
- [x] Verify: 5,540-file card cold cull 13.6 s; warm re-cull 0.17 s (target <1.5 s/1000 ✓); 595 bursts, 1,124 picks; sidecars valid (LR read-check pending user)

## Test data safety (user instruction 2026-07-21)
- [x] Corpus copied to /home/pc/fd_testdata/100EOSR5 — ALL tool runs use the copy, originals in /home/pc/temp are off-limits

## M2 — GUI browse + harvest  ✅ first cut 2026-07-22
- [x] fd-core::pipeline Engine: scan thread + worker pool + priority job queue (Full>Preview>Thumb>Score), cache-backed scoring
- [x] fd-gui (eframe 0.31/glow): Card Overview grid (virtualized, burst stacks, cull rings), Burst View (main image + filmstrip with score/flag badges), zoom fit/100% with progressive PREVIEW→FULL, pan
- [x] Keyboard: arrows/Enter/Esc/N · P/X/U/1-5/0 · Ctrl+Enter accept top-2 · Ctrl+X reject burst · O sort · Ctrl+Z undo · Ctrl+H harvest · ? help
- [x] Texture LRU budgets (1500 thumbs / 8 previews / 2 fulls)
- [x] Harvest dialog: XMP + copy picks (background thread, progress)
- [x] Session persistence (.fd-session.tsv, debounced autosave)
- [x] --screenshot self-test mode; verified overview/burst/full-card renders on real corpus (595 stacks streaming)
- [ ] User acceptance pass (interactive feel, keyboard flow) — needs eyes on screen
- Note: DPMS-off suppresses repaints (found during headless testing) — irrelevant in real use
## M3 — Click-ROI + NCC tracking + ROI ranking  ✅ first cut 2026-07-22
- [x] fd-core::track: pyramid-free coarse-to-fine NCC (stride 8→2→1, ±96px window), dual template (original + adaptive), confidence = NCC peak; unit tests (translation follow, lost-track low conf)
- [x] Engine track jobs: bidirectional from seed, per-frame TrackPoint events (normalized coords + conf + ROI sharpness), newer request aborts older
- [x] GUI: click on image = set track point; ROI box overlay (green/amber/red by confidence) on main + mini-box on strip; ROI scores in badges; ranking prefers confident ROI scores (lost frames sink); Ctrl+Enter uses ROI ranking
- [x] ~45 ms/frame track (decode 5 ms + NCC 40 ms); 16-frame burst < 1 s
- [x] Verified on real corpus: bird burst tracked, box rendered, strip re-ranked (screenshots)
- [ ] Later: keyframe correction UI (re-click mid-burst merges), confidence sparkline, threshold slider, top-K full-res refinement
## UX — control system + reviewable harvest  ✅ 2026-08-03
- [x] Fix: GUI drew no text at all (eframe `default-features = false` dropped `default_fonts`); guard test added
- [x] One `Action` enum + `App::perform` dispatcher; keyboard, menu and toolbar share it; `App::enabled` drives greying-out
- [x] `KEYMAP` as data + tests: no duplicate key+modifier, every action labelled, every bound action listed
- [x] Menu bar File/Edit/View/Burst/Help with shortcut text; state shown via `Button::selected` (egui's fonts have no checkmark glyph)
- [x] Context toolbar per view; star rating widget; tracking confidence chip + Clear track
- [x] Status bar (folder, counts, scan/score progress, cursor position); on-image overlay text removed, only PICK/REJECT badge remains
- [x] `File > Open Folder…` via rfd + `App::open_dir` engine swap; Ctrl+S / Ctrl+Q; `wants_keyboard_input` guard
- [x] Confirm modal for Ctrl+X reject-all; empty-folder state with Open Folder button
- [x] fd-core::recipe — Recipe/PlannedAction/Op with evidence fields, `check` (read-only), `execute` (dry_run writes nothing), Report; 6 unit tests
- [x] Harvest rebuilt as Configure → Review → Running → Done; review table with per-row toggles, filters, status column; recipe saved to `<folder>/cull-recipe.json`; File > Open Recipe…
- [x] `fd apply <recipe.json> [--dry-run] [--verbose]` CLI parity
- [x] `--build-recipe` / `--open-harvest` self-test flags (alongside existing `--screenshot`/`--open-burst`)
- [ ] User acceptance pass — menus/toolbar feel, and whether the review table's columns are the right ones

## Image source option + EXIF auto-rotate — 2026-09-04
- [x] fd-core: `formats::Source {Embedded, Full}` + `FromStr`; `extract_preview` → `extract_jpeg(meta, source)`
- [x] fd-core: `cache::file_key(meta, source)` (`-full` suffix; embedded key unchanged)
- [x] fd-core: `decode::orient` (EXIF 2–8) + `Luma/Rgba::upright`; `FileMeta::display_dims`; unit tests
- [x] fd-core: pipeline — `Engine::start(.., source)`, unified Thumb/Preview/Full arm with upright RGBA, score keyed by source, track luma upright
- [x] golden tests: `Source::Full` assertions for JPG / CR3 / CR2
- [x] fd-cli: `fd cull --source embedded|full`
- [x] fd-gui: `--source` flag; `Action::SetSource`; View menu radio; status-bar label; help text; rescan on change
- [x] fd-gui: upright display — `display_dims` for 1:1, ROI box side by long edge, aspect-fit thumbs (overview + filmstrip)
- [x] docs: README, CHANGES, ARCHITECTURE, in-app help
- [x] verify: cargo test; `fd cull --dry-run` baseline unchanged; `--source full` on test copy; GUI screenshots of a portrait burst (fit / track / inspect / full source)
- [x] zoom/pan kept when flipping frames within a burst; mouse-wheel zoom about the cursor (`App.zoom: Option<f32>`); Shift+wheel resizes the focus measuring area (`TrackRequest.roi_frac`, box drawn at true size, burst re-tracked)

## Keyframe pins + stronger sharpness detector — 2026-09-04
- [x] A: `TrackRequest.seeds` (pins); `run_track` wavefront per pin bounded by nearest-pin ownership; GUI pin on click, thick box + "pinned" chip, Clear track drops all; Shift+Left/Right = prev/next burst; zoom carries over between bursts
- [x] B1: `examples/sharpbench.rs` — synthetic native-scale blur/noise/shift/exposure ground truth, 4 targets (w4096/w2048/w1638/native512), 10 metrics, tracked `--burst` mode
- [x] B2: gated normalized Laplacian in `score.rs`; `SCORE_VERSION` in cache key; 4 new tests
- [x] B3: native-res ROI refinement — benchmarked and rejected (no gain in discrimination/jitter, 3-10x noise inflation, +decode per frame); not built
- [x] docs: README, CHANGES, ARCHITECTURE, help; review + lessons

## Eye sharpness (camera eye box + edge acuity), busy feedback, HiDPI, auto-brighten — 2026-09-05
- [x] C: busy indicator — `Engine::busy()`, progress cursor + "working… N" in the status bar, "measuring…" chip
- [x] D: HiDPI — start maximized, auto `set_zoom_factor` from monitor height (`--ui-zoom` override), Ctrl+/− documented
- [x] F: auto-brighten toggle (View, `B`) — display-only LUT in the decode worker, `--brighten` flag
- [x] E1: Canon AFInfo2 eye box (`formats/canon.rs`, tiff accessors, CR3 CMT3), `AfBox`, `af_box_eye`, `orient_norm`, golden tests
- [x] E2: `decode_luma_crop` (turbojpeg lossless crop): 0.24–0.27 s per 45 MP frame, bit-identical to the full decode
- [x] E3: `eye.rs` — pupil segmentation (darkest seed, area-jump stop, size cap, solidity, retries) + radial 10–90% edge width (p75), 5 unit tests
- [x] E4: `sharpbench --eyes` montages on the owl (15/16 pupils) and a portrait pale-eyed burst (9/12); ranking agrees with eye crops at both ends
- [x] E5: pipeline camera pins + two-phase track (NCC, then parallel native crops) + burst radius consistency; GUI eyes/kind, circle overlay, chip, px badges, auto-track on open, Use Camera Eye Points, recipe basis
- [x] docs: README, CHANGES, ARCHITECTURE, help; review + lessons
- [x] Non-destructive: `fd-session.json` (flags, stars, pins; TSV migrated), harvest copies picks to `<folder>_keepers` with XMP next to the copies, in-place sidecars opt-in, no delete ever
- [x] G: focus peaking (`K`, View, `--peaking`) — display-only overlay in the decode worker (`decode::peaking_overlay`); focus coverage (`eye::coverage`, same test) on every `TrackPoint`, `N% in focus` badge/chip, *Sort by Focus Coverage* as third `O` mode
- [x] Small UX: Shift+wheel over the contact sheet resizes thumbnails (0.5–2x); `+`/`-` step stars without wrap (Edit > Rating)
- [x] 2026-09-08 resume: ARCHITECTURE bullet for coverage/peaking + display-only flags; help-text sentence fixed; `--peaking` screenshot self-test

## M4 — HEIF, DPRAW, packaging (never deletes: rejects are recorded only)
- [ ] Packaging: plan written in `tasks/packaging_20260905.md` (CI matrix + screenshot smoke test, cargo-packager msi/dmg/AppImage, signing later); awaiting go-ahead to implement
## M5 — (stretch) web build

## Review

### 2026-08-03 — control system + recipe harvest
Two requests: make the controls discoverable for general use, and make
harvest a two-step recipe → execute process whose middle step is reviewable.

**What was built.** Menu bar, context toolbar and status bar, all dispatching
through a single `Action`/`perform` layer so the three input routes cannot
diverge. Harvest now builds a JSON recipe carrying, per file, the burst, rank
within burst, sharpness, ROI sharpness, tracker confidence and a plain-language
reason — reviewed in a table with per-row toggles before anything is written.

**Verified.** 22 tests pass. End-to-end on a 15-file copy: recipe named 11
sidecars, `--dry-run` wrote nothing, execute produced exactly those 11 files
and the 15 originals stayed byte-identical (md5). Recipe building is
deterministic across runs. Screenshots confirm overview, burst, review table
and empty-folder chrome.

**Two lessons.** (1) egui's embedded fonts have no `✓`/`✗`/`←` glyphs — they
render as empty boxes; `Button::selected` shows state without any glyph.
(2) The GUI idles at ~1 fps when there is no background work, so screenshot
self-tests need timeouts scaled to `--shot-frames`, not to wall-clock guesses;
a too-short timeout looks exactly like a hang.

**Not done.** Interactive acceptance (menu feel, whether the review columns are
the right ones) needs eyes on the screen. The overview frame-count badge is
still white-on-bright and hard to read on pale thumbnails — pre-existing, left
alone as out of scope.

### 2026-09-04 — image source option + upright display
Two requests: let the user process from the embedded preview or the full image
(JPG and CR3), and show images the right way up.

**What was built.** `formats::Source` threads from the CLI flag / View menu into
`Engine::start`, `extract_jpeg` and the cache key; `decode::orient` re-lays-out
RGBA and tracking luma for all eight EXIF cases so the GUI, clicks and track
points share one upright space; thumbnails are letterboxed; the 1:1 view uses
`display_dims`.

**Verified.** 28 tests pass (fd-core 24 incl. Full-source golden checks on the
real JPG/CR3/CR2 fixtures; fd-gui 4). Full-source cull of the whole test copy:
6 min 51 s cold (5,350 fresh), 0.2 s warm. `fd cull --dry-run` on the 5,540-file
test copy still hits every cached score (0 fresh) — old keys unchanged. On a
12-file copy of a portrait burst, `--source full` scored 12 fresh in 0.95 s and
the re-run hit all 12 `-full` keys; rankings within bursts matched embedded.
Screenshots on that copy: portrait frames upright in fit, tracked, inspect and
overview; status bar shows `source: full image`.

**Decisions.** Thumbs always embedded (a 1/8-scale full decode is still 1024 px);
global score luma not rotated (metric is rotation-neutral; keeps cache valid);
the source is per launch, not persisted.

### 2026-09-04 — pins + sharpness detector
Requests: let the user correct the focus point per frame; the sharpness
scores "barely differ" between frames.

**Pins.** `TrackRequest.seeds`; one NCC wavefront per pin, each frame owned by
its nearest pin. Clicking a drifted frame fixes it and its neighbours without
losing earlier pins. Plus Shift+Left/Right between bursts and zoom carry-over.

**Detector.** Built `examples/sharpbench.rs` first (lesson of 2026-07-21).
Findings on three real frames, working resolution 2048 px, D = sharp/blur
ratio for a 2 px native Gaussian blur, Dn1 = sharp+noise8 / blur1+noise8:

| metric | D(2px) | Dn1 | N8 | flat-noise/sharp |
|---|---|---|---|---|
| Sobel/var (old) | 1.17-1.22 | 1.04-1.06 | 1.01 | 8-12 |
| Laplacian/var | 2.45-3.62 | 1.20-1.43 | 1.15-1.52 | 116-247 (0.3-0.7 gated) |
| SML/MAD | 1.49-1.80 | 1.06-1.15 | 1.16-1.72 | 11-17 |
| two-scale ratio | 1.16-1.20 | 1.04-1.06 | 1.01 | 6-9 |
| top-15% Sobel | 1.06-1.11 | 1.02-1.03 | 1.00 | 2-3 |
| pre-smoothed Sobel | 1.10-1.14 | 1.03-1.04 | 1.01 | 3-5 |

Native-res crop (512 px): Laplacian D(1px) 5.5-8.8 but N8 14-49 and Dn1 1.0
(noise wins); Sobel D(1px) 1.3-2.0 with N8 1.6-2.6 — no better SNR than the
working-res Laplacian (D(1px) 1.33-1.52, jitter 4-9%). Half-native (4096 px)
Laplacian: D(1px) 2.3-3.0 but jitter 20-26% — same SNR again. So: ship the
gated Laplacian at working resolution, skip the native path.

**Verified.** 28 tests; `fd cull` cold 20 s / warm 0.2 s (new `-s2` keys);
owl burst ROI spread 1.10x -> 3.55x with the tracked eye, top frames confirmed
by native eye crops; screenshots show pinned box/chip and the new badges.

### 2026-09-05 — eye sharpness, busy cursor, HiDPI, auto-brighten
Request: "sharpness should be based on the eye only (needs detecting the
pupil); correlation with perceived sharpness is not high" — plus busy
feedback, 4K scaling, auto-brighten, and user override when Eye-AF misses.

**Found.** Every R5 II file records the Eye-AF frame (MakerNote AFInfo2, +Y
up, stored orientation); on the owl it is a 246 px box on the pupil. That
gives the eye per frame for free; a click pin overrides it.

**Built.** Lossless native crop at the eye (0.25 s/frame, 3 in parallel),
pupil segmentation, 64-ray 10–90% rim width (p75) as the ROI score, burst
radius consistency, camera pins as implicit pins, two-phase track events, GUI
circle/chip/badges in px, Burst > Use Camera Eye Points.

**Segmentation took four iterations** (see lessons): percentile thresholds
fail when the pupil is 4% (owl) or 0.3% (pale-eyed bird) of the window; the
working recipe is grow-from-darkest-seed, stop before the first area jump,
cap the radius at half the eye box, gate on solidity, retry from the next
seed, and check radii across the burst.

**Verified.** 41 tests. Owl burst: 15/16 pupils, crispest frames by eye rank
first and the softest (0842) last, one motion-blurred frame correctly has no
pupil. Portrait pale-eyed burst: 9/12 pupils (r 12 px), the +Y-up mapping
holds on orientation 8. Screenshot shows circles, `Eye (camera) · N px` chip
and px badges after auto-track on open.

