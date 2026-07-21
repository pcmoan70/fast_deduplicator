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
## M3 — Click-ROI + NCC tracking + ROI ranking
## M4 — HEIF, DPRAW, trash-rejects, packaging
## M5 — (stretch) web build

## Review
(filled in per milestone)
