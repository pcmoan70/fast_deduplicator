# Architecture

*Updated: 2026-07-21 (M0 complete; M1+ sections describe the approved design).*

## Core principle

**Never decode RAW sensor data on the hot path.** Every Canon still format
embeds JPEGs sufficient for browsing, ranking and sharpness scoring:

| Format | Thumb | Preview | Full-size JPEG | Where |
|---|---|---|---|---|
| CR3 (ISO-BMFF) | `THMB` 160×120 | `PRVW` 1620×1080 | trak sample in `mdat` (native res) | offsets from `moov` (first ~256 KB) |
| CR2 (TIFF) | IFD1 | IFD0 strip (large) | — | offsets in first few KB |
| JPEG | — | itself | itself | — |
| HEIF (M4) | thumbnail item | — | — | `meta` box |

A 25 MB CR3 costs ~130 KB (header scan) + ~370 KB (preview) of I/O.
Measured on the reference corpus (5,540 R5 II files): 0.83 s full-card
header scan, 6,647 files/s.

## Workspace layout

```mermaid
graph LR
  CLI[fd-cli · binary fd] --> CORE[fd-core · headless library]
  GUI[fd-gui · eframe/wgpu M2] --> CORE
  subgraph fd-core
    F[formats: cr3, cr2, jpeg, tiff, exif]
    P[pipeline: scan, schedule M2]
    S[score: sharpness M1]
    T[track: ncc M3]
    B[burst grouping M1]
    C[cache: sqlite M1]
    O[output: xmp, harvest M1]
  end
```

- `fd-core/src/io.rs` — `ReadRange` trait (the wasm/Vfs seam) + `FileReader`
  with cached header chunk and byte-count accounting.
- `fd-core/src/formats/tiff.rs` — endian-aware IFD walker shared by CR2,
  JPEG APP1 and CR3 `CMTx` boxes.
- `fd-core/src/formats/cr3.rs` — ISO-BMFF box walk: `moov` → Canon uuid
  (`CMT1` IFD0, `CMT2` ExifIFD, `THMB`, `CTBO`), trak tables for the
  full-size JPEG sample, `CTBO` entry 2 for the `PRVW` uuid byte range
  (parsed lazily at extraction via `locate_prvw_jpeg`).
- Timestamps: `DateTimeOriginal` + `SubSecTimeOriginal` → centiseconds
  (`Timestamp.unix_centis`); at 30 fps neighbors differ by 3–4 centis.

## Scan pipeline (stages stream; UI never waits)

```mermaid
flowchart LR
  A[A: walk + header parse\n~130 KB/file] --> C[C: burst grouping\nserial+time+filenum]
  A --> B[B: preview pread + decode\n~370 KB/file]
  B --> D[scoring / thumbs]
  C --> D
  D --> E[D: lazy full-res on zoom]
```

Planned concurrency (M2): coordinator thread with priority job queue
(`Visible > NearViewport > ActiveBurst > Prefetch > Background`), separate
I/O pool (sized for card-reader latency, 8–16 in flight), rayon CPU pool,
single SQLite writer; cancellation via generation counter.

## Scoring & tracking (M1/M3)

- Sharpness: Tenengrad + variance-of-Laplacian on luma, 3 pyramid scales,
  normalized by local RMS contrast; ROI patch from the tracked point, else
  max-over-tiles global score.
- Tracking: pyramidal NCC template match, bidirectional from the clicked
  frame, dual-template drift control, per-frame confidence; re-click adds a
  keyframe.

## Cache (M1)

SQLite (WAL, one writer thread): `files`, `thumbs`, `scores`, `bursts`,
`roi_tracks`, `decisions`. Key = `(size, mtime, xxh3 of first 64 KB)` —
survives remounts/drive letters. Reopen of a scanned card ≈ 1 s.

## Outputs (M1/M2)

XMP sidecars (never touch originals), templated copy of picks
(RAW + sidecar + paired JPEG atomically), rejects to OS trash only, deletes
run last so cancel is clean.
