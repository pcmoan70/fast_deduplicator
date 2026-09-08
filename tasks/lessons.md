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

## 2026-08-01 — `default-features = false` silently removed the GUI's fonts
`fd-gui` pinned `eframe = { default-features = false, features = ["glow",
"wayland", "x11"] }`. That list looks like "just the backends", but eframe's
default set also carries `default_fonts`; without it egui's
`FontDefinitions::default()` returns `empty()` and every glyph draws as
nothing — no panic, no log, no missing-glyph boxes. The window read as
black, and buttons collapsed to nubs because their labels measured zero.
Rule: when disabling default features, diff the crate's actual `default = [...]`
list and re-add every non-backend feature you still need; a feature that
degrades to silence rather than a compile error needs a test pinning it.

## 2026-08-03 — egui's embedded fonts have no checkmark or arrow glyphs
Built a toolbar with "✓ Pick", "✗ Reject", "← Back" and a "✔" menu marker.
All four rendered as empty boxes: egui's default font set (Hack,
Ubuntu-Light, NotoEmoji, emoji-icon-font) covers ★/☆/·/… but not U+2713,
U+2717, U+2714 or U+2190. Nothing warns — a missing glyph just draws tofu.
Rule: in egui, restrict UI text to ASCII plus glyphs already proven to
render in this app, and show widget state with `Button::selected` or a
drawn shape rather than a symbol character. Verify any new glyph in a
screenshot before relying on it.

## 2026-08-03 — A slow render loop looks exactly like a hang
The `--screenshot` self-test appeared to hang: no output, process alive,
main thread asleep. It was running correctly at ~1 fps — with no background
decode work the app only repaints on the screenshot path's own
`request_repaint`, so 60 frames took ~60 s and my 20–25 s timeouts killed it
mid-run. Two wasted diagnostic rounds went into "where is the deadlock".
Rule: before hunting a deadlock, prove the loop is stopped rather than slow —
print progress per iteration and scale the timeout to the work requested
(here, `--shot-frames`). Also: piping a hung process through `head` hides
its output; redirect to a file instead.

## 2026-09-04 — A per-pixel pass over a 45 MP frame is never free
The first EXIF-rotate was a plain gather loop with a `match` per pixel; on
the 8192x5464 RGBA inspect frame it took 1.9 s — three times the JPEG decode
it followed — and the inspect screenshot self-test silently fell back to
PREVIEW because the full-res texture never arrived in time. A 128x128 tiled
transpose with the per-row stride hoisted out of the inner loop brought it to
~0.2-0.3 s. Rule: any new full-resolution pixel pass gets timed on a real
45 MP frame before it ships, and a screenshot self-test that can show a
fallback state (PREVIEW vs FULL) must be read for that chip, not just for
"something rendered".

## 2026-09-04 — Fix the frequency response before buying resolution
The sharpness scores "barely differed" (16 frames within 2.47-2.72). The
obvious fix was measuring at native resolution (a lossless JPEG crop at the
tracked point). The benchmark said otherwise: the real defect was that a 3x3
Sobel has no response at the band where a 4x-downsampled fine blur lives, so
swapping it for a 5-point Laplacian tripled the discrimination at zero cost,
while native-resolution measurement gave the same discrimination-to-jitter
ratio with 3-10x the noise inflation and a decode per frame. Rule: when a
detector looks weak, benchmark its frequency response against synthetic
ground truth on real frames first; only buy resolution (or compute) if the
benchmark shows the cheap formula is actually limited. Keep the benchmark in
the repo (`examples/sharpbench.rs`) so the next change is measured too.

## 2026-09-05 — Segment the pupil by growth, not by percentile
Four rounds to a robust pupil segmentation. Fixed percentile thresholds
(Otsu, p10-based) depend on how much of the window the pupil fills — 4% on
an owl, 0.3% on a pale-eyed bird — so each order of thresholds fixed one bird
and broke the other and the synthetic test. What worked: seed at the darkest
pixel near the centre, grow the region over rising thresholds, stop before
the first area jump measured against the blob's own perimeter, cap the
radius at half the eye box (a size prior the camera gives us), reject
non-solid fits (an eye-ring arc), retry from the next seed outside the
rejected region, and check radii across the burst. Also: a fallback value
that looks like a measurement (a nominal circle of 0.3 x window) hid the
failures in the benchmark table for two rounds — print "no pupil", never a
plausible-looking number, for a failed measurement.
