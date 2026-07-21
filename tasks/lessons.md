# Lessons

## 2026-07-21 — Per-scale normalization inverts multi-scale sharpness
Planned metric was 3-scale contrast-normalized Tenengrad. Empirically (numpy
replication) the downsampled scales score *blurred* content HIGHER — 2x
decimation re-sharpens the residual spectrum — so the mix diluted a 2:1
discrimination down to 1.06:1. Rule: when a metric combines pyramid levels,
normalize all levels by the *full-resolution* contrast, or don't mix; always
validate a scoring formula on synthetic blur pairs before shipping it.

## 2026-07-21 — Verify performance claims at target scale, not toy scale
The first full-card score pass took 212 s (cold I/O artifact) while the
9-file sample suggested ~5 ms/image. Re-running at full scale under
controlled conditions gave 13.6 s. Rule: benchmark the real workload on the
real corpus before optimizing; and don't chase a number that only appeared
once — reproduce it first.

## 2026-07-21 — Destructive-tool development needs sacrificial data
User instruction: never point the tool at original photos. Working copy:
/home/pc/fd_testdata/100EOSR5. Delete/trash features (M4) must be developed
and tested exclusively against copies.
