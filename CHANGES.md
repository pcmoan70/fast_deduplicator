# Changes

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
