# Plan: cross-platform builds and one-click installers (M4 packaging)

*Written 2026-09-05. Status: plan only, nothing implemented. Depends on a push to
GitHub to verify, so it runs as its own step when approved.*

## Goal

A non-technical user opens the GitHub Releases page, downloads one file for their
computer, double-clicks it, and `fast_deduplicator` runs. Three downloads per release:

| platform | file | install experience |
|---|---|---|
| Windows 10/11 x64 | `fast_deduplicator-<ver>-windows-x64.msi` | installer, Start-menu entry, uninstaller |
| macOS 12+ (Intel + Apple Silicon) | `fast_deduplicator-<ver>-macos-universal.dmg` | drag the app to Applications |
| Linux x86_64 | `fast_deduplicator-<ver>-linux-x86_64.AppImage` (+ `.deb`) | mark executable, double-click |

The `fd` CLI ships inside the same packages (installed next to the app on
Windows/Linux, inside the `.app` bundle on macOS) for power users; it is not a
separate download.

## Why this is feasible today

- Pure Rust except two C libraries that compile from source during `cargo build`:
  libjpeg-turbo (turbojpeg-sys via cmake, NASM optional for SIMD) and SQLite
  (rusqlite `bundled`). No system packages to install at runtime.
- File dialogs: rfd uses the XDG desktop portal on Linux (no GTK), AppKit on
  macOS, the Win32 dialog on Windows — all already in the lock file.
- eframe with the `glow` backend runs on all three; the `x11`/`wayland` features
  are Linux-only extras and inert elsewhere.
- No platform-specific code, no hard-coded separators; paths are `PathBuf`
  throughout; the cache and session files live next to the images.

## Step 1 — repository changes (small, do first, testable locally)

1. `crates/fd-gui/src/main.rs`: `#![windows_subsystem = "windows"]` behind
   `#[cfg(not(debug_assertions))]` so release builds open no console window on
   Windows; keep stderr usage for the `--screenshot` self-test (it still works,
   output goes nowhere on Windows release builds — acceptable).
2. App identity: an icon (`assets/icon.png` 1024², from which the packager derives
   `.ico`/`.icns`), a bundle id `com.pcmoan.fast_deduplicator`, product name
   "fast_deduplicator", version taken from the workspace `Cargo.toml`.
3. `Cargo.toml` `[package.metadata.packager]` for `cargo-packager`: product name,
   identifier, icons, `formats = ["msi", "dmg", "appimage", "deb"]`, binaries
   `fd-gui` (main) and `fd` (extra), category Photography, license MIT.
4. `scripts/fetch_fixtures.sh` already skips missing local corpora; the golden
   tests skip without fixtures, so CI runs the pure-logic tests only (unchanged).
5. README "Installing and running": replace the build-from-source-only text with
   the three download lines and the per-OS first-launch notes below; keep the
   build-from-source section for developers.

## Step 2 — CI workflow (`.github/workflows/ci.yml`)

Matrix `ubuntu-24.04`, `windows-2022`, `macos-14`; on every push and pull request:

1. Checkout, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache`.
2. Tool prerequisites: cmake is on every runner; add NASM (`apt install nasm`,
   `choco install nasm`, `brew install nasm`) so libjpeg-turbo builds its SIMD
   paths (about 2x faster decode; without it the build still succeeds).
3. `cargo build --release --workspace` and `cargo test --release --workspace`.
4. **GUI smoke test per OS**: run
   `fd-gui <tiny fixture folder> --screenshot out.png --shot-frames 20`
   (a two-file folder committed under `fixtures/ci/`, small JPGs with an MPF
   preview, or generated at build time from the existing test images if they can
   be redistributed). Linux runs it under `xvfb-run`; Windows and macOS runners
   have a desktop session. The step fails if the PNG is missing or all-black
   (a 30-line check with the `image` crate as a tiny `examples/checkshot.rs`, or
   `identify -format %[mean]`). This is what catches "opens no window on
   Windows" before a user does.
5. Upload the three release binaries as workflow artifacts (zip/tar.gz) so any
   commit can be tried on any OS without a release.

Cost: about 20–30 runner minutes per push across the three OSes; free on a public
repository, well inside the private-repo free tier.

## Step 3 — release workflow (`.github/workflows/release.yml`)

Triggered by pushing a tag `v*`:

1. Same matrix and prerequisites; `cargo install cargo-packager --locked`.
2. macOS: build both `x86_64-apple-darwin` and `aarch64-apple-darwin`, `lipo`
   them into a universal binary, then package the `.dmg` (cargo-packager does
   the `.app` bundle; a plain `hdiutil` step makes the dmg if the packager's dmg
   needs extra tooling).
3. Windows: `msi` via cargo-packager (WiX toolset is preinstalled on the runner).
4. Linux: `appimage` and `deb` via cargo-packager (it downloads `linuxdeploy`).
5. Generate SHA-256 checksums, create the GitHub Release with `softprops/
   action-gh-release`, attach the six files (three installers, three checksums),
   release notes taken from the matching `CHANGES.md` section.
6. Version bump procedure: edit `version` in the workspace `Cargo.toml`, add the
   `CHANGES.md` section, commit, `git tag vX.Y.Z`, push tag.

Alternative considered: `cargo-dist`. Excellent for CLIs (shell/PowerShell
installers, Homebrew) but its GUI story is thinner (no dmg/AppImage); the app is
GUI-first, so `cargo-packager` is the better fit. Revisit if its dmg step proves
fragile.

## Step 4 — first-launch friction and how to remove it

| platform | without signing | with signing | cost |
|---|---|---|---|
| Windows | SmartScreen "Windows protected your PC" → More info → Run anyway | no warning once the certificate has reputation | Azure Trusted Signing ~$10/month, or an OV certificate ~$200–400/year |
| macOS | "cannot be opened because the developer cannot be verified" → right-click → Open (Sonoma+: System Settings → Privacy & Security → Open Anyway) | Gatekeeper opens it directly | Apple Developer Program $99/year (Developer ID certificate + notarization with `notarytool` in the workflow) |
| Linux AppImage | must be marked executable once (`chmod +x` or Properties → Permissions) | n/a | none |

Recommendation: ship unsigned first with the README explaining the one-time
click-through, add Windows signing when there are outside users, add Apple
notarization when a Mac user asks. Both are workflow secrets and a step each.

## Step 5 — verification before the first public release

- CI green on all three runners including the screenshot smoke test.
- Download each installer from the draft release onto a clean machine or VM and
  do the user path: install, open a folder of test JPGs (copies, never
  originals), open a burst, click the eye, harvest with `--dry-run` semantics
  (recipe review, execute nothing). Windows and macOS VMs are the two I cannot
  run from here; the first release is a draft until someone ticks those.
- Check the HiDPI zoom on a Windows 150% scale factor and a Retina Mac: egui
  already gets the OS scale factor, and the app adds its own screen-height
  zoom, so both should land near the same physical size; confirm.
- Confirm the session file and cache are created next to the images on all
  three (paths with drive letters, spaces, non-ASCII folder names).

## Known platform points to watch

- Card readers on Windows appear as drive letters; `collect_paths` walks them
  fine, but the `Open Folder…` dialog should default to the last folder, not
  the executable's directory (check `rfd` behaviour on Windows).
- Windows Defender occasionally scans large JPG reads; first scan of a card may
  be slower than on Linux. Nothing to do, just expectation-setting.
- macOS: the app must ask for permission to read removable volumes on first use
  (TCC prompt); fine, but the bundle needs a `NSDesktopFolderUsageDescription`-
  style string only if the sandbox is enabled, which it is not.
- Auto-update: not in scope. Users re-download. Reconsider when there is a
  steady user base (cargo-packager has an updater, Tauri-style).

## Effort and order

Half a day for steps 1–2 (the CI matrix and smoke test are the real value: every
future change is proven to open a window on three OSes), half a day for step 3
and the first tagged draft release, then whatever the Windows and macOS test
sessions turn up. Signing is separate and optional.

## Open decisions for the owner

1. Public or private repository (affects Actions minutes and whether releases are
   publicly downloadable).
2. Buy signing now or after first outside users.
3. Whether the `fd` CLI should also get `cargo-dist`-style shell installers for
   power users, or stay bundled only.
