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

## M4 — HEIF, DPRAW, trash-rejects, packaging
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
