# fast_deduplicator

Super-fast culling tool for Canon EOS memory cards. Scans thousands of RAW
files in seconds, groups burst sequences, ranks frames by sharpness at a
point you click once (e.g. a bird's eye, tracked through the burst), and
harvests the keepers: XMP ratings, copy picks, trash rejects.

*Updated: 2026-07-22 — status: M0–M3 first cuts done: format core, burst
grouping + sharpness ranking (CLI), GUI browse/cull/harvest, click-once
eye tracking with ROI ranking. Next: HEIF, trash-rejects, packaging (M4).*

## GUI

```bash
fd-gui /path/to/card
```

Card Overview shows every burst as a stack (green ring = culled). Enter
opens a burst: filmstrip with sharpness badges, arrows navigate, `P`/`X`
pick/reject with auto-advance, `1–5` stars, `Ctrl+Enter` accepts the top-2
and rejects the rest, `N` jumps to the next unculled burst, `Z` toggles
100% zoom (preview first, full-res swaps in). `Ctrl+H` opens Harvest
(XMP sidecars + copy picks). `?` shows all keys. Flags autosave to a
session file; reopening the folder resumes where you left off.

**Click-and-track:** click the critical point (e.g. the bird's eye) on the
main image. The point is tracked through the whole burst and every frame
gets a box colored by tracking confidence (green/amber/red); the filmstrip
re-ranks by sharpness *at that point*. Re-click anywhere to move the
track; frames where the track was lost rank last.

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

fd cull DIR --dry-run            # group bursts, rank by sharpness, list picks
fd cull DIR --top 2 --xmp        # write XMP sidecars (rating 3) for top 2/burst
fd cull DIR --copy-to KEEPERS    # copy picks (+sidecars) to a folder
fd bench FILE                    # per-stage timings for one file
```

Measured on the reference card (5,540 files): cold cull 13.6 s, re-cull
with warm cache 0.17 s. RAW+JPEG pairs are treated as one image. Canon
JPGs are scored from their embedded 1620px MPF preview (~300 KB read
instead of a 45 MP decode).

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
