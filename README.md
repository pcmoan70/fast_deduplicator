# fast_deduplicator

Super-fast culling tool for Canon EOS memory cards. Scans thousands of RAW
files in seconds, groups burst sequences, ranks frames by sharpness at a
point you click once (e.g. a bird's eye, tracked through the burst), and
copies the keepers to a folder of your choice. Originals are never touched.

*Updated: 2026-09-08 — status: M0–M3 done: format core, burst grouping +
sharpness ranking (CLI), GUI with full menu/toolbar control system,
click-once eye tracking with ROI ranking, a reviewable recipe harvest, a
selectable image source (embedded preview or full image) and upright display
by EXIF orientation. Next: HEIF, packaging (M4).*

## GUI

```bash
fd-gui /path/to/card     # or just `fd-gui` and use File > Open Folder…
```

Everything is on the menu bar — **File, Edit, View, Burst, Help** — with each
keyboard shortcut printed beside its entry, so the fast path teaches itself.
Actions that do not apply right now are greyed rather than hidden. Below it a
toolbar carries the frequent actions for the current view, and a status bar
reports counts, scan/score progress and where you are.

Card Overview shows every burst as a stack (green ring = culled; Shift+wheel
resizes the thumbnails). Enter opens a
burst: filmstrip with sharpness badges, Left/Right move between frames and
Shift+Left/Right (or Up/Down) jump to the previous/next burst keeping the zoom
level, `P`/`X` pick/reject
with auto-advance, `1–5` stars (`+`/`-` step one star, stopping at 5 and 0), `Ctrl+Enter` accepts the top 2 and rejects the
rest, `Ctrl+X` rejects the whole burst (asks first), `N` jumps to the next
unculled burst, `Z` toggles 100% zoom (preview first, full-res swaps in). The
mouse wheel zooms about the cursor, up to 8x; drag to pan. The spot you zoomed
into is kept as you flip through the burst's frames, so neighbours are compared
at the same place.
`Ctrl+Z` undoes. `?` shows all keys. Flags, stars and focus pins autosave to
`fd-session.json` in the folder (plain JSON, the only file the app writes
there); reopening the folder resumes where you left off.

**Eye sharpness:** the camera's Eye-AF frame (Canon MakerNote) is read from
every file, so when you open a burst the focus point lands on the eye in each
frame the camera detected, without a click. The pupil is then segmented at full
resolution and the sharpness is the width of its edge in pixels (lower is
sharper): a pupil rim 4 px wide is in focus, 9 px is not, and a directional
smear is flagged as motion. Badges, the toolbar chip and the harvest evidence
show that width. Frames where no pupil can be found keep the overall score and
drop below the measured ones.

**Helping the camera:** the camera's eye frame is always drawn as a dashed box,
green while it is the focus point in use. When Eye-AF missed the point, click the
right spot on that frame. That adds a pin: the pin (solid, thick, green)
overrides the camera's box on its frame, which turns orange, and frames with no
box follow their nearest pin by tracking. Right-click the image, press
Backspace or use the toolbar's `Unpin` to cancel the override on that frame;
`Clear Pins` cancels all of them. If the camera
tracked the wrong subject through a whole burst, turn off *Burst > Use Camera
Eye Points* and place pins yourself. Shift+wheel sets the search area used
around a tracked point when there is no camera box. Frames where the track
was lost rank last.

**Inspect mode (`I`):** the full-resolution embedded JPEG at 1:1, automatically
centered on the tracked focus point of *each* frame — flip through the burst
with Left/Right and the eye stays pinned in place while the rest of the frame
moves around it, which makes the sharpness difference between neighbouring
frames obvious. Without a track it centers on the image middle (the chip says
so — click the subject to get the real thing). Neighbour frames are pre-decoded
so flipping lands on full resolution, and drag still pans if you want to look
around. `I` or `Z` leaves the mode.

**Image source (View menu):** by default every file is browsed, scored and
tracked from the camera's embedded preview (a Canon JPG's 1620 px MPF appendix,
a CR3's PRVW). *View > Source: Full Image* works from the full image instead:
the JPG itself, or a CR3's native-resolution embedded JPEG, decoded to ~2000 px,
so sharpness comes from the real pixels rather than the camera's re-encoded
preview. It is much slower, because every file is read in full. Scores from the
two sources are cached separately and are not comparable with each other.
Switching reopens the folder: flags are kept, undo history is not. The status
bar says `source: full image` while it is active, and `fd-gui DIR --source full`
starts that way. Thumbnails always come from the preview and 100% zoom always
from the full image, whatever the setting.

**Browsing aids:** `K` (View > Focus Peaking) paints every pixel that passes
the focus test red, like the camera's MF peaking, on the preview and the 1:1
view; the eye score "N% in focus" counts those pixels inside the focus area,
and *Sort by Focus Coverage* (`O` cycles time / sharpness / coverage) ranks by
it. Dark frames are lifted automatically for viewing (on by
default; `B` or View > Auto-brighten Dark Images turns it off, `--no-brighten`
starts without it). Frames that already reach the highlights are left alone,
and scores never see it. The UI scales itself to the
window (1080 px tall is 1x, a maximized 4K window is 2x) and Ctrl+Plus /
Ctrl+Minus / Ctrl+0 adjust it.
The cursor turns into the busy indicator and the status bar says `working…`
while anything is decoding or measuring.

Images are shown the right way up: the EXIF orientation is applied to
thumbnails, previews, full-resolution views and the tracking coordinates, so a
portrait shot is portrait everywhere and a click lands where you clicked.

## Harvest is two steps, and you check the middle one

Nothing in this tool is destructive: originals are never moved, changed or
deleted, and a reject is only a flag in `fd-session.json`. Harvest copies the
picked images (RAW plus paired JPEG) to a folder of your choice, by default a
`<folder>_keepers` sibling, with an XMP rating sidecar next to each copy so
Lightroom or darktable pick the stars up there. Writing sidecars next to the
originals is an opt-in checkbox.

`Ctrl+H` does not write anything. It builds a **recipe** — a JSON list of every
intended edit — and shows it as a table you read before committing:

| run | file | operation | burst | rank | sharp | track | why / status |
|---|---|---|---|---|---|---|---|
| ☑ | 4P4A1413.JPG | copy to …_keepers + rate 3★ | 1 | 1/6 | 10.6 | 0.82 | picked: rank 1/6 in burst 1 by eye edge acuity (camera eye point) |
| ☐ | 4P4A1410.JPG | skip | 1 | 3/6 | 9.9 | — | rejected: rank 3/6 in burst 1 by overall sharpness |

Every row carries the evidence behind the decision, so you can confirm the
burst grouping, the ranking and the tracker detected what you intended —
*before* a single file is touched. Rejected frames appear as `skip` rows, so
what is being left behind is reviewable too, not just what is acted on. A
precondition pass flags problems (missing source, a sidecar that would be
overwritten, a name collision in the copy destination) in the same table.

Untick any row to drop it. Then **Execute** runs only the ticked rows, through
the same code path whether it comes from the GUI or the CLI.

The recipe is written to `<folder>/cull-recipe.json` and can be saved
elsewhere, inspected in a text editor, reopened later with **File > Open
Recipe…**, or executed headlessly:

```bash
fd apply cull-recipe.json --dry-run   # report exactly what would change
fd apply cull-recipe.json             # do it
```

`--dry-run` writes nothing at all; it reports the same set of actions the real
run would perform.

## Why it's fast

RAW sensor data is never decoded for browsing or scoring. Canon files embed
JPEGs at three sizes (160px thumb, 1620px preview, full resolution); the
scanner reads only the byte ranges it needs — ~1–2 MB per 25 MB CR3.
Measured on a real EOS R5 Mark II card dump (5,540 files, 103 GB):
**header scan of the whole card in 0.9 s** (~6,600 files/s).

## Formats

CR3 (incl. C-RAW), CR2, JPEG — parsed with bespoke minimal readers.
HEIF (.HIF) planned (M4). Dual Pixel RAW tolerated (previews unaffected).
The *Full Image* source uses the JPG file itself or a CR3's native-resolution
embedded JPEG; CR2 has none and falls back to its IFD0 preview. EXIF orientation
is read from all three and applied on display, and the Eye-AF frame is read
from Canon's MakerNote (AFInfo2) in JPG and CR3.

## Usage (CLI, current state)

```bash
fd scan /path/to/card            # metadata table + timing summary
fd scan DIR --all                # one row per file
fd scan DIR --dump-previews OUT  # extract 1620px embedded previews
fd scan DIR --dump-fullsize OUT  # extract full-res embedded JPEGs

fd cull DIR --dry-run            # group bursts, rank by sharpness, list picks
fd cull DIR --top 2 --xmp        # write XMP sidecars (rating 3) for top 2/burst
fd cull DIR --copy-to KEEPERS    # copy picks (+sidecars) to a folder
fd cull DIR --source full        # score from the full image, not the preview

fd apply RECIPE.json --dry-run   # step 2 of harvest: report, write nothing
fd apply RECIPE.json --verbose   # ...and do it, one line per action

fd bench FILE                    # per-stage timings for one file
```

Measured on the reference card (5,540 files): cold cull ~20 s, re-cull
with warm cache 0.2 s. RAW+JPEG pairs are treated as one image. Sharpness is
contrast-normalized Laplacian energy at the working resolution, chosen by a
benchmark on real frames (see ARCHITECTURE.md); scores only mean something
relative to each other within a burst. Canon
JPGs are scored from their embedded 1620px MPF preview (~300 KB read
instead of a 45 MP decode). `--source full` reads and entropy-decodes the whole
file instead (1/4 DCT scale, 2048 px) and keeps its scores under separate cache
keys, so the two modes never mix: cold full-source cull of the same card took
6 min 51 s (5,350 JPGs, ~95 GB read); re-cull with warm cache 0.2 s.

## Installing and running

There are no prebuilt binaries yet (packaging is M4), so you build from source.
Two binaries come out of one build: `fd-gui` (the app) and `fd` (the CLI).

### What you need on every platform

| Requirement | Why |
|---|---|
| **Rust 1.81 or newer** | eframe/egui minimum; install via [rustup](https://rustup.rs) |
| **A C compiler** | SQLite is compiled from source (`rusqlite` bundled) |
| **CMake** | libjpeg-turbo is compiled from vendored source |
| **NASM** | that build runs with `REQUIRE_SIMD=ON`, which fails without an assembler |

CMake and NASM are needed *even though* nothing links libjpeg-turbo
dynamically — the crate always builds it from source with default features.
See [Avoiding CMake and NASM](#avoiding-cmake-and-nasm) if that is awkward.

### Linux

```bash
# Debian / Ubuntu / Mint
sudo apt install build-essential cmake nasm pkg-config

# Fedora
sudo dnf install gcc gcc-c++ cmake nasm pkgconf-pkg-config

# Arch
sudo pacman -S base-devel cmake nasm
```

At runtime the app needs OpenGL and either X11 or Wayland. These are loaded
dynamically, so no `-dev` packages are required to build — on any normal
desktop install they are already present (`libgl1`, `libx11-6`,
`libxkbcommon0` on Debian-family).

File dialogs (`File > Open Folder…`, *Save recipe as…*) go through the **XDG
desktop portal**, not GTK:

```bash
sudo apt install xdg-desktop-portal xdg-desktop-portal-gtk   # or -kde, -wlr
```

Without a portal the app still runs normally — pass the folder on the command
line instead; only the dialogs are unavailable.

### macOS

```bash
xcode-select --install        # C compiler
brew install cmake nasm
```

Nothing extra at runtime: the window and the file dialogs use system
frameworks. Both Apple Silicon and Intel build from the same source.

### Windows

1. **Visual Studio Build Tools** with the *Desktop development with C++*
   workload — this provides the MSVC C compiler.
2. **CMake** ([cmake.org/download](https://cmake.org/download/)) and **NASM**
   ([nasm.us](https://www.nasm.us/)), both on `PATH`. Both are also carried by
   winget, Chocolatey and Scoop if you prefer. The NASM installer does not add
   itself to `PATH`; add its install directory manually or the libjpeg-turbo
   build fails with an assembler error.
3. **Rust** via [rustup](https://rustup.rs), `x86_64-pc-windows-msvc` toolchain.

Nothing extra at runtime. The binaries are `target\release\fd-gui.exe` and
`target\release\fd.exe`.

### Build and run

```bash
cargo build --release            # fd-gui and fd in target/release/

./target/release/fd-gui /path/to/card    # open a card directly
./target/release/fd-gui                  # then File > Open Folder…

./scripts/fetch_fixtures.sh      # sample files for golden tests
cargo test --release
```

### Avoiding CMake and NASM

If you would rather not install them, either point the build at a system
libjpeg-turbo (3.0 or newer):

```bash
TURBOJPEG_SOURCE=pkg-config cargo build --release
```

…or drop the C dependency entirely by editing `crates/fd-core/Cargo.toml` and
changing `default = ["turbo"]` to `default = []`. JPEG decoding then uses the
pure-Rust `zune-jpeg` path, which is slower but needs no C toolchain for the
image codec (SQLite still wants a C compiler).

### Tested platforms

Built and run on **Linux Mint 22.1, X11, kernel 6.8** — the screenshots and
timings in this README come from there. The macOS and Windows instructions
follow from the dependencies' documented requirements but have **not** been
exercised on those platforms; treat them as a starting point and please report
what actually happens.

## Roadmap

- M1: burst grouping, sharpness scoring, SQLite cache, XMP sidecars, auto-cull
- M2: GUI (egui/wgpu) — progressive grid, filmstrip, keyboard culling, harvest
- M3: click-once eye tracking (NCC), sharpness-at-ROI ranking
- M4: HEIF, packaging for Windows/macOS/Linux
- M5 (stretch): browser build (wasm)

See `ARCHITECTURE.md` for design details and `CHANGES.md` for history.
