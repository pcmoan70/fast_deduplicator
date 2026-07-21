# Changes

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
