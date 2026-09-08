//! Cull recipes: the two-step harvest.
//!
//! Step one turns a culling session into a `Recipe` — a plain JSON list of
//! every edit the tool intends to make, each carrying the evidence behind it
//! (which burst, what rank, what sharpness, how confident the tracker was).
//! Nothing is written. The user reads that back, in the GUI table or in a text
//! editor, and confirms the automated steps detected what was intended.
//!
//! Step two executes it. `check` reports problems without touching anything;
//! `execute` performs the enabled actions through the existing `output`
//! helpers, and in `dry_run` mode reports what it would do and writes nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::output;

/// Bumped when the on-disk shape changes incompatibly.
pub const RECIPE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    pub version: u32,
    /// Folder the paths in `actions` are relative to.
    pub folder: PathBuf,
    pub created_unix: u64,
    pub settings: Settings,
    pub actions: Vec<PlannedAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// How many frames per burst were kept.
    pub top_n: usize,
    /// Burst grouping gap, seconds.
    pub gap_secs: u64,
    pub write_xmp: bool,
    pub copy_to: Option<PathBuf>,
}

/// What to do with one file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Write an XMP sidecar next to the original.
    WriteXmp { rating: u8 },
    /// Copy the file (and any existing sidecar) into `dest`; with `rating`
    /// above 0 an XMP sidecar is written next to the copy, never next to the
    /// original.
    Copy {
        dest: PathBuf,
        #[serde(default)]
        rating: u8,
    },
    /// Deliberately left alone. Present so the review shows what is being
    /// dropped, not only what is acted on.
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlannedAction {
    /// Relative to `Recipe::folder`.
    pub file: PathBuf,
    #[serde(flatten)]
    pub op: Op,
    /// Cleared in review to drop a single action without rebuilding.
    pub enabled: bool,
    // ---- evidence: why the tool chose this ----
    /// 1-based burst number, matching what the UI displayed.
    pub burst: usize,
    pub burst_size: usize,
    /// 1-based rank within the burst under the ranking that was in effect.
    pub rank: usize,
    pub sharpness: f32,
    /// Sharpness at the tracked point, when a track was running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roi_sharpness: Option<f32>,
    /// Tracker confidence at this frame, when a track was running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_confidence: Option<f32>,
    /// Plain-language justification, shown in the review table.
    pub reason: String,
}

impl PlannedAction {
    pub fn op_label(&self) -> String {
        match &self.op {
            Op::WriteXmp { rating } => format!("rate {rating}★"),
            Op::Copy { dest, rating: 0 } => format!("copy to {}", dest.display()),
            Op::Copy { dest, rating } => format!("copy to {} + rate {rating}★", dest.display()),
            Op::Skip => "skip".into(),
        }
    }

    /// Absolute source path within `folder`.
    pub fn source(&self, folder: &Path) -> PathBuf {
        folder.join(&self.file)
    }
}

impl Recipe {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> Result<Recipe, serde_json::Error> {
        serde_json::from_str(s)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = self
            .to_json()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    pub fn load(path: &Path) -> std::io::Result<Recipe> {
        let text = std::fs::read_to_string(path)?;
        Recipe::from_json(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Actions that would actually run.
    pub fn active(&self) -> impl Iterator<Item = &PlannedAction> {
        self.actions
            .iter()
            .filter(|a| a.enabled && !matches!(a.op, Op::Skip))
    }

    pub fn active_count(&self) -> usize {
        self.active().count()
    }

    pub fn now_unix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// A problem found by `check`, keyed to the action's index in `Recipe::actions`.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub action: usize,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Will not run, or will lose data.
    Error,
    /// Will run, but the user should know.
    Warning,
}

/// Read-only precondition pass. Writes nothing; the review table shows the
/// result so problems are visible *before* the execute step.
pub fn check(recipe: &Recipe) -> Vec<Issue> {
    let mut issues = Vec::new();
    // Two enabled actions writing the same destination name would collide;
    // copy_pick would silently suffix the second. Worth surfacing.
    let mut copy_targets: HashMap<PathBuf, usize> = HashMap::new();

    for (i, a) in recipe.actions.iter().enumerate() {
        if !a.enabled || matches!(a.op, Op::Skip) {
            continue;
        }
        let src = a.source(&recipe.folder);
        if !src.exists() {
            issues.push(Issue {
                action: i,
                severity: Severity::Error,
                message: "source missing".into(),
            });
            continue;
        }
        match &a.op {
            Op::WriteXmp { .. } => {
                if output::sidecar_path(&src).exists() {
                    issues.push(Issue {
                        action: i,
                        severity: Severity::Warning,
                        message: "overwrites existing sidecar".into(),
                    });
                }
            }
            Op::Copy { dest, .. } => {
                if dest.exists() && !dest.is_dir() {
                    issues.push(Issue {
                        action: i,
                        severity: Severity::Error,
                        message: format!("destination is not a folder: {}", dest.display()),
                    });
                    continue;
                }
                let name = src.file_name().unwrap_or_default();
                let target = dest.join(name);
                if target.exists() {
                    issues.push(Issue {
                        action: i,
                        severity: Severity::Warning,
                        message: "name already in destination; copy will be suffixed".into(),
                    });
                }
                if let Some(prev) = copy_targets.insert(target.clone(), i) {
                    issues.push(Issue {
                        action: i,
                        severity: Severity::Warning,
                        message: format!(
                            "same destination name as action {}; copy will be suffixed",
                            prev + 1
                        ),
                    });
                }
            }
            Op::Skip => {}
        }
    }
    issues
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub action: usize,
    pub file: PathBuf,
    /// What happened, or what would happen under `dry_run`.
    pub detail: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub dry_run: bool,
    pub sidecars: usize,
    pub copied: usize,
    pub skipped: usize,
    pub outcomes: Vec<Outcome>,
}

impl Report {
    pub fn failures(&self) -> impl Iterator<Item = &Outcome> {
        self.outcomes.iter().filter(|o| o.error.is_some())
    }

    pub fn summary(&self) -> String {
        let verb = if self.dry_run { "would write" } else { "wrote" };
        let failed = self.failures().count();
        let mut s = format!(
            "{verb} {} sidecars, {} copies ({} skipped)",
            self.sidecars, self.copied, self.skipped
        );
        if failed > 0 {
            s.push_str(&format!(" — {failed} failed"));
        }
        s
    }
}

/// Run the enabled actions. With `dry_run` nothing touches the filesystem;
/// the report still lists exactly what each action would have done.
///
/// `progress` is called with (done, total) after each action.
pub fn execute(
    recipe: &Recipe,
    dry_run: bool,
    mut progress: impl FnMut(usize, usize),
) -> Report {
    let mut report = Report {
        dry_run,
        ..Default::default()
    };
    let total = recipe.actions.len();

    for (i, a) in recipe.actions.iter().enumerate() {
        if !a.enabled || matches!(a.op, Op::Skip) {
            report.skipped += 1;
            progress(i + 1, total);
            continue;
        }
        let src = a.source(&recipe.folder);
        let mut outcome = Outcome {
            action: i,
            file: a.file.clone(),
            detail: String::new(),
            error: None,
        };

        match &a.op {
            Op::WriteXmp { rating } => {
                let sc = output::sidecar_path(&src);
                outcome.detail = format!("sidecar {} rating {rating}", sc.display());
                if !dry_run {
                    match output::write_sidecar(&src, *rating, None) {
                        Ok(_) => report.sidecars += 1,
                        Err(e) => outcome.error = Some(e.to_string()),
                    }
                } else {
                    report.sidecars += 1;
                }
            }
            Op::Copy { dest, rating } => {
                outcome.detail = format!("copy to {}", dest.display());
                if !dry_run {
                    match output::copy_pick(&src, dest) {
                        Ok(p) => {
                            outcome.detail = format!("copied to {}", p.display());
                            report.copied += 1;
                            if *rating > 0 {
                                match output::write_sidecar(&p, *rating, None) {
                                    Ok(_) => report.sidecars += 1,
                                    Err(e) => outcome.error = Some(e.to_string()),
                                }
                            }
                        }
                        Err(e) => outcome.error = Some(e.to_string()),
                    }
                } else {
                    report.copied += 1;
                    if *rating > 0 {
                        report.sidecars += 1;
                    }
                }
            }
            Op::Skip => unreachable!("skips are filtered above"),
        }

        report.outcomes.push(outcome);
        progress(i + 1, total);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fd-recipe-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn action(file: &str, op: Op) -> PlannedAction {
        PlannedAction {
            file: PathBuf::from(file),
            op,
            enabled: true,
            burst: 1,
            burst_size: 3,
            rank: 1,
            sharpness: 6.3,
            roi_sharpness: None,
            track_confidence: None,
            reason: "rank 1/3 by sharpness".into(),
        }
    }

    fn recipe_in(folder: &Path, actions: Vec<PlannedAction>) -> Recipe {
        Recipe {
            version: RECIPE_VERSION,
            folder: folder.to_path_buf(),
            created_unix: 1_700_000_000,
            settings: Settings {
                top_n: 2,
                gap_secs: 60,
                write_xmp: true,
                copy_to: None,
            },
            actions,
        }
    }

    /// The rating travels with the copy: the sidecar lands next to the copy
    /// and the source folder stays untouched.
    #[test]
    fn copy_with_rating_writes_sidecar_in_destination_only() {
        let dir = tmpdir("copyrate");
        std::fs::write(dir.join("a.JPG"), b"jpeg").unwrap();
        let dest = dir.join("keepers");
        let r = recipe_in(&dir, vec![action("a.JPG", Op::Copy { dest: dest.clone(), rating: 4 })]);
        let report = execute(&r, false, |_, _| {});
        assert_eq!(report.failures().count(), 0);
        assert!(dest.join("a.JPG").exists() && dest.join("a.xmp").exists());
        assert!(!dir.join("a.xmp").exists(), "source folder must stay untouched");
        assert_eq!((report.copied, report.sidecars), (1, 1));
    }

    #[test]
    fn json_round_trips() {
        let r = recipe_in(
            Path::new("/cards/100EOSR5"),
            vec![
                action("a.JPG", Op::WriteXmp { rating: 3 }),
                action("b.JPG", Op::Copy { dest: PathBuf::from("/keepers"), rating: 0 }),
                action("c.JPG", Op::Skip),
            ],
        );
        let json = r.to_json().unwrap();
        assert_eq!(Recipe::from_json(&json).unwrap(), r);
        // The evidence must survive the round trip - it is the whole point.
        assert!(json.contains("\"reason\""));
        assert!(json.contains("\"rank\""));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tmpdir("dry");
        std::fs::write(dir.join("a.JPG"), b"x").unwrap();
        let before = std::fs::read_dir(&dir).unwrap().count();

        let r = recipe_in(&dir, vec![action("a.JPG", Op::WriteXmp { rating: 3 })]);
        let report = execute(&r, true, |_, _| {});

        assert!(report.dry_run);
        assert_eq!(report.sidecars, 1, "dry run still reports the intent");
        assert!(!dir.join("a.xmp").exists(), "dry run must not write");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), before);
    }

    #[test]
    fn execute_writes_only_what_was_planned() {
        let dir = tmpdir("exec");
        std::fs::write(dir.join("a.JPG"), b"x").unwrap();
        std::fs::write(dir.join("b.JPG"), b"y").unwrap();

        let r = recipe_in(
            &dir,
            vec![
                action("a.JPG", Op::WriteXmp { rating: 4 }),
                action("b.JPG", Op::Skip),
            ],
        );
        let report = execute(&r, false, |_, _| {});

        assert_eq!(report.sidecars, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.failures().count(), 0);
        assert!(dir.join("a.xmp").exists());
        assert!(!dir.join("b.xmp").exists(), "skips must stay untouched");
        assert!(std::fs::read_to_string(dir.join("a.xmp"))
            .unwrap()
            .contains("xmp:Rating=\"4\""));
    }

    #[test]
    fn disabled_actions_do_not_run() {
        let dir = tmpdir("disabled");
        std::fs::write(dir.join("a.JPG"), b"x").unwrap();
        let mut r = recipe_in(&dir, vec![action("a.JPG", Op::WriteXmp { rating: 3 })]);
        r.actions[0].enabled = false;

        let report = execute(&r, false, |_, _| {});
        assert_eq!(report.sidecars, 0);
        assert!(!dir.join("a.xmp").exists());
    }

    #[test]
    fn check_flags_missing_source_and_sidecar_overwrite() {
        let dir = tmpdir("check");
        std::fs::write(dir.join("a.JPG"), b"x").unwrap();
        std::fs::write(dir.join("a.xmp"), b"old").unwrap();

        let r = recipe_in(
            &dir,
            vec![
                action("a.JPG", Op::WriteXmp { rating: 3 }),
                action("gone.JPG", Op::WriteXmp { rating: 3 }),
            ],
        );
        let issues = check(&r);

        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].severity, Severity::Warning);
        assert!(issues[0].message.contains("existing sidecar"));
        assert_eq!(issues[1].severity, Severity::Error);
        assert!(issues[1].message.contains("source missing"));
        // check() is read-only.
        assert_eq!(std::fs::read_to_string(dir.join("a.xmp")).unwrap(), "old");
    }

    #[test]
    fn progress_reports_every_action() {
        let dir = tmpdir("progress");
        std::fs::write(dir.join("a.JPG"), b"x").unwrap();
        let r = recipe_in(
            &dir,
            vec![
                action("a.JPG", Op::WriteXmp { rating: 3 }),
                action("b.JPG", Op::Skip),
            ],
        );
        let mut seen = Vec::new();
        execute(&r, true, |done, total| seen.push((done, total)));
        assert_eq!(seen, vec![(1, 2), (2, 2)]);
    }
}
