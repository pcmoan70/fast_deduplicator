# Changes

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
