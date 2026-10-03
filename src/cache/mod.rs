//! Build caches grafted into trees, so a new tree doesn't build from cold.
//!
//! A repo declares its cache directories (see [`declare`]). Each cache is
//! grafted in one of two modes:
//!
//! - `clone`: the main checkout's directory is cloned into the tree, large files
//!   hardlinked and small ones copied (see [`clone`]).
//! - `never`: left cold.
//!
//! workforest only ever replaces or deletes a cache path that git ignores and
//! tracks nothing under, so a mistaken declaration can't destroy source.

mod clone;
pub mod declare;
pub mod doctor;
mod glob;
mod walk;

use std::fmt;
use std::fs::{self, File, FileTimes};
use std::path::Path;
use std::time::SystemTime;

pub use declare::declared;

use crate::error::{Context, Result, bail};
use crate::git;

/// How a cache path is grafted into a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Clone,
    Never,
}

impl Mode {
    fn parse(mode: &str) -> Option<Mode> {
        match mode {
            "clone" => Some(Mode::Clone),
            "never" => Some(Mode::Never),
            _ => None,
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` rather than `write_str`, so that column widths apply.
        f.pad(match self {
            Mode::Clone => "clone",
            Mode::Never => "never",
        })
    }
}

/// One cache path a repo declares.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Relative to the repo root, normalized, and checked by
    /// [`declare::valid_path`].
    pub path: String,
    pub mode: Mode,
    /// Globs of files that `clone` copies whatever their size.
    pub always_copy: Vec<String>,
    /// Where the entry was declared, for `cache paths`.
    pub origin: String,
}

/// Print each warning about a repo's declarations.
pub fn warn(warnings: &[String]) {
    for warning in warnings {
        eprintln!("workforest: {warning}");
    }
}

/// Graft every cache that the repo with its main worktree at `source` declares
/// into `tree`, printing a line for each. Files of `link_min` bytes and up are
/// hardlinked. `force` replaces caches the tree already has. A cache that can't
/// be grafted is left as it was, with a warning: the tree works without it.
pub fn graft_tree(tree: &Path, source: &Path, link_min: u64, force: bool) {
    let declared = declared(source);
    warn(&declared.warnings);
    for entry in &declared.entries {
        match graft(tree, source, entry, link_min, force) {
            Ok(Some(note)) => println!("  cache {}: {note}", entry.path),
            Ok(None) => {}
            Err(err) => eprintln!("workforest: cache {}: {err}", entry.path),
        }
    }
}

/// Graft one cache into `tree`, returning what happened, if anything.
fn graft(
    tree: &Path,
    source: &Path,
    entry: &Entry,
    link_min: u64,
    force: bool,
) -> Result<Option<String>> {
    match entry.mode {
        Mode::Clone => graft_clone(tree, source, entry, link_min, force).map(Some),
        Mode::Never => Ok(None),
    }
}

fn graft_clone(
    tree: &Path,
    source: &Path,
    entry: &Entry,
    link_min: u64,
    force: bool,
) -> Result<String> {
    let (src, dst) = (source.join(&entry.path), tree.join(&entry.path));
    check_replaceable(tree, &entry.path, true)?;
    let replacing = dst.symlink_metadata().is_ok();
    if replacing && !force {
        return Ok("already there, left alone".to_owned());
    }
    // Make sure there is something to graft before giving up what the tree has.
    let kept = if replacing {
        "the tree keeps its own"
    } else {
        "starting cold"
    };
    if !src.is_dir() {
        return Ok(format!("the main checkout has none; {kept}"));
    }
    let parent = dst.parent().unwrap_or(tree);
    fs::create_dir_all(parent).context(format!("could not create {}", parent.display()))?;
    if !clone::same_filesystem(&src, parent) {
        return Ok(format!(
            "{} is on another filesystem, so it can't be hardlinked; {kept}",
            src.display()
        ));
    }
    // Clone beside the cache and move it into place, so that an interrupted
    // graft never leaves a partial cache that looks whole.
    let staging = staging_path(&entry.path);
    discard_staging(tree, &staging)?;
    let staged = tree.join(&staging);
    let grafted = clone::clone_dir(&src, &staged, source, link_min, &entry.always_copy)
        .context(format!("could not clone {}", src.display()))
        .and_then(|cloned| {
            if replacing {
                remove(tree, &entry.path)?;
            }
            fs::rename(&staged, &dst).context(format!("could not move it to {}", dst.display()))?;
            Ok(cloned)
        });
    if grafted.is_err() {
        let _ = fs::remove_dir_all(&staged);
    }
    let cloned = grafted?;
    // A tree planted just now is newer than anything grafted into it, but one
    // planted or edited before the main checkout's last build is not.
    let marked = match cloned.newest.map(|newest| mark_older_changed(tree, newest)) {
        None => 0,
        Some(Ok(marked)) => marked,
        Some(Err(err)) => {
            let _ = remove(tree, &entry.path);
            bail!("{err}; dropped the graft, so that its output can't pass as fresh");
        }
    };
    let mut note = format!(
        "grafted from the main checkout: {}, {} hardlinked, {} copied",
        count(cloned.files, "file"),
        human_bytes(cloned.linked),
        human_bytes(cloned.copied)
    );
    if marked > 0 {
        note.push_str(&format!(
            "; marked the tree's {} older than it as changed",
            count(marked, "file")
        ));
    }
    if cloned.links_to_main > 0 {
        note.push_str(&format!(
            "; {} into the main checkout, so a build that writes through one changes it",
            count(cloned.links_to_main, "symlink")
        ));
    }
    Ok(note)
}

/// Mark as changed now each file of the tree at `tree` that git tracks or
/// would track and that is no newer than `newest`, returning how many. Build
/// tools take output newer than its sources for built from them, so output
/// grafted from a build that came after the tree's checkout or edits would
/// otherwise pass as fresh, though built from the main checkout's sources.
fn mark_older_changed(tree: &Path, newest: SystemTime) -> Result<u64> {
    let Some(files) = git::files(tree) else {
        bail!("could not list the files of {}", tree.display());
    };
    let now = FileTimes::new().set_modified(SystemTime::now());
    let mut marked = 0;
    for file in files {
        let path = tree.join(file);
        // Only regular files: a symlink's target may be outside the tree, and
        // a tracked file may have been deleted.
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if !meta.is_file() || meta.modified().is_ok_and(|modified| modified > newest) {
            continue;
        }
        // Setting explicit times needs ownership, not write access.
        File::open(&path)
            .and_then(|file| file.set_times(now))
            .context(format!("could not mark {} as changed", path.display()))?;
        marked += 1;
    }
    Ok(marked)
}

/// Where the cache at `path` is cloned before it is moved into place: beside
/// it, so on the same filesystem, under a name that is workforest's own.
fn staging_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/.{name}.workforest-graft"),
        None => format!(".{path}.workforest-graft"),
    }
}

/// Delete what an interrupted graft left at `staging` in `tree`. The name is
/// workforest's own, so only files git tracks there stop it.
fn discard_staging(tree: &Path, staging: &str) -> Result<()> {
    let dir = tree.join(staging);
    if dir.symlink_metadata().is_err() {
        return Ok(());
    }
    if git::tracks(tree, staging) {
        bail!("git tracks files under {staging}, so it can't stage a graft there");
    }
    let removed = if dir.is_symlink() || !dir.is_dir() {
        fs::remove_file(&dir)
    } else {
        fs::remove_dir_all(&dir)
    };
    removed.context(format!("could not remove {}", dir.display()))
}

/// Delete the cache at `path` in `tree`, if it is safe to.
pub fn remove(tree: &Path, path: &str) -> Result<()> {
    let dst = tree.join(path);
    let Ok(meta) = dst.symlink_metadata() else {
        return Ok(());
    };
    check_replaceable(tree, path, !meta.is_symlink())?;
    let removed = if meta.is_dir() {
        fs::remove_dir_all(&dst)
    } else {
        fs::remove_file(&dst)
    };
    removed.context(format!("could not remove {}", dst.display()))
}

/// Fail unless git ignores `path` in `tree` and tracks nothing under it, so
/// that replacing or deleting it can't lose source. `dir` says whether the
/// path is, or will be, a directory rather than a symlink.
fn check_replaceable(tree: &Path, path: &str, dir: bool) -> Result<()> {
    if git::tracks(tree, path) {
        bail!("git tracks files under {path}, so it isn't a cache");
    }
    if !git::ignores(tree, path, dir) {
        if dir {
            bail!("git doesn't ignore {path}, so it isn't a cache; add it to .gitignore");
        }
        bail!(
            "git doesn't ignore a symlink at {path}; list it in .gitignore without a trailing slash"
        );
    }
    Ok(())
}

/// What the tree at `tree` has of the cache `entry` declares: how much of it
/// is still hardlinked to the main checkout, or why there's nothing to count.
pub fn describe(tree: &Path, entry: &Entry) -> String {
    let path = tree.join(&entry.path);
    match entry.mode {
        Mode::Never => "never".to_owned(),
        Mode::Clone if path.is_symlink() || !path.is_dir() => "cold".to_owned(),
        Mode::Clone => match check_replaceable(tree, &entry.path, true) {
            Err(err) => err.to_string(),
            Ok(()) => {
                let usage = clone::usage(&path);
                format!(
                    "{}, {} hardlinked, {} own",
                    count(usage.files, "file"),
                    human_bytes(usage.shared),
                    human_bytes(usage.own)
                )
            }
        },
    }
}

/// `n` and `noun`, plural unless `n` is 1.
fn count(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// `bytes` in the largest binary unit that keeps it at 1 or more.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
