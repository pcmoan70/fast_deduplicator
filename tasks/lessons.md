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
