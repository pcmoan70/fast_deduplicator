# fast_deduplicator

Super-fast culling tool for Canon EOS memory cards. Scans thousands of RAW
files in seconds, groups burst sequences, ranks frames by sharpness at a
point you click once (e.g. a bird's eye, tracked through the burst), and
harvests the keepers: XMP ratings, copy picks, trash rejects.

*Updated: 2026-07-21 — status: M0 (format core + CLI) done.*

## Why it's fast

RAW sensor data is never decoded for browsing or scoring. Canon files embed
JPEGs at three sizes (160px thumb, 1620px preview, full resolution); the
scanner reads only the byte ranges it needs — ~1–2 MB per 25 MB CR3.
Measured on a real EOS R5 Mark II card dump (5,540 files, 103 GB):
**header scan of the whole card in 0.9 s** (~6,600 files/s).

## Formats

CR3 (incl. C-RAW), CR2, JPEG — parsed with bespoke minimal readers.
HEIF (.HIF) planned (M4). Dual Pixel RAW tolerated (previews unaffected).

## Usage (CLI, current state)

```bash
fd scan /path/to/card            # metadata table + timing summary
fd scan DIR --all                # one row per file
fd scan DIR --dump-previews OUT  # extract 1620px embedded previews
fd scan DIR --dump-fullsize OUT  # extract full-res embedded JPEGs
```

## Building

```bash
cargo build --release            # binary at target/release/fd
./scripts/fetch_fixtures.sh      # sample files for golden tests
cargo test --release
```

## Roadmap

- M1: burst grouping, sharpness scoring, SQLite cache, XMP sidecars, auto-cull
- M2: GUI (egui/wgpu) — progressive grid, filmstrip, keyboard culling, harvest
- M3: click-once eye tracking (NCC), sharpness-at-ROI ranking
- M4: HEIF, packaging for Windows/macOS/Linux
- M5 (stretch): browser build (wasm)

See `ARCHITECTURE.md` for design details and `CHANGES.md` for history.
