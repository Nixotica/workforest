//! Build caches grafted into trees, so a new tree doesn't build from cold.
//!
//! A repo declares its cache directories (see [`declare`]), and some build
//! systems' are known without a declaration (see [`ecosystem`]). Each cache
//! is grafted in one of two modes:
//!
//! - `clone`: the repo's seed is cloned into the tree, large files hardlinked
//!   and small ones copied (see [`clone`]).
//! - `never`: left cold.
//!
//! A repo's seed (see [`seed`]) lives under the forest root, and takes the
//! main checkout's caches when they have newer output. Trees donate theirs
//! back to it (see [`donate`]), so that trees planted later graft a warm one.
//! The main checkout's caches are only ever read.
//!
//! workforest only ever replaces or deletes a tree's cache path that git
//! ignores and tracks nothing under, so a mistaken declaration can't destroy
//! source.

mod cargo;
mod clone;
pub mod declare;
pub mod doctor;
mod donate;
mod ecosystem;
mod glob;
pub mod seed;
mod walk;

use std::fmt;
use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use clone::usage;
pub use declare::declared;
pub use donate::{Say, Transfer, donate_tree};
use seed::{Record, Seed};

use crate::error::{Context, Error, Result, bail};
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
    /// Whether the entry is built in (see [`ecosystem`]) rather than declared.
    pub built_in: bool,
    /// Whether each top-level directory of the cache is a cache of its own,
    /// so that a seed can take some of them and not others.
    pub parts: bool,
}

/// Print each warning about a repo's declarations.
pub fn warn(warnings: &[String]) {
    for warning in warnings {
        eprintln!("workforest: {warning}");
    }
}

/// A tree, and where its caches come from and go back to.
pub struct Planting {
    pub tree: PathBuf,
    /// The main checkout of the tree's repo, whose caches are only ever read.
    pub source: PathBuf,
    /// The repo's seed (see [`seed`]).
    pub seed: PathBuf,
    /// Where the tree records what it grafted from the seed.
    pub record: PathBuf,
}

impl Planting {
    /// The tree `repo` of the forest at `forest`, planted from `source`, with
    /// seeds under the forest root `root`.
    pub fn new(root: &Path, forest: &Path, repo: &str, source: &Path) -> Planting {
        Planting {
            tree: forest.join(repo),
            source: source.to_path_buf(),
            seed: seed::dir(root, source),
            record: forest.join(seed::RECORDS).join(repo),
        }
    }
}

/// The seed `planting` grafts from and donates to, opened and locked the first
/// time it is needed.
fn open_seed<'a>(slot: &'a mut Option<Seed>, planting: &Planting) -> Result<&'a mut Seed> {
    if slot.is_none() {
        *slot = Some(Seed::open(&planting.seed, &planting.source)?);
    }
    Ok(slot.as_mut().expect("just opened"))
}

/// Graft every cache that the tree's repo declares into it from the repo's
/// seed, printing a line for each, once the seed has taken whatever output of
/// the main checkout's is newer than its own. Files of `link_min` bytes and up
/// are hardlinked. `force` replaces caches the tree already has. A cache that
/// can't be grafted is left as it was, with a warning: the tree works without
/// it.
pub fn graft_tree(planting: &Planting, link_min: u64, force: bool) {
    let declared = declared(&planting.source);
    warn(&declared.warnings);
    let mut record = Record::read(&planting.record);
    let mut seed = None;
    for entry in &declared.entries {
        match graft(planting, &mut seed, &mut record, entry, link_min, force) {
            Ok(notes) => {
                for note in notes {
                    println!("  cache {}: {note}", entry.path);
                }
            }
            Err(err) => eprintln!("workforest: cache {}: {err}", entry.path),
        }
    }
    if let Err(err) = record.write() {
        eprintln!("workforest: {err}");
    }
}

/// Graft one cache into the tree, returning what happened, if anything.
fn graft(
    planting: &Planting,
    seed: &mut Option<Seed>,
    record: &mut Record,
    entry: &Entry,
    link_min: u64,
    force: bool,
) -> Result<Vec<String>> {
    if entry.mode == Mode::Never {
        return Ok(Vec::new());
    }
    let main = planting.source.join(&entry.path);
    let seeded = planting.seed.join(&entry.path);
    // Every repo of a build system gets its built-in entry, so the many
    // without its cache, such as those that build elsewhere, pass quietly.
    if entry.built_in && !main.is_dir() && !seeded.is_dir() {
        return Ok(Vec::new());
    }
    check_replaceable(&planting.tree, &entry.path, true)?;
    let dst = planting.tree.join(&entry.path);
    let replacing = dst.symlink_metadata().is_ok();
    if replacing && !force {
        return Ok(vec!["already there, left alone".to_owned()]);
    }
    let kept = if replacing {
        "the tree keeps its own"
    } else {
        "starting cold"
    };
    if !main.is_dir() && !seeded.is_dir() {
        return Ok(vec![format!("the main checkout has none; {kept}")]);
    }
    let seed = open_seed(seed, planting)?;
    let mut notes = Vec::new();
    if main.is_dir() {
        match donate::take_from_main(seed, &planting.source, entry, link_min) {
            Ok(note) => notes.extend(note),
            Err(err) => eprintln!(
                "workforest: cache {}: the seed could not take the main checkout's: {err}",
                entry.path
            ),
        }
    }
    if !seeded.is_dir() {
        notes.push(format!("the seed has none; {kept}"));
        return Ok(notes);
    }
    let parent = dst.parent().unwrap_or(&planting.tree);
    fs::create_dir_all(parent).context(format!("could not create {}", parent.display()))?;
    if !clone::same_filesystem(&seeded, parent) {
        notes.push(format!(
            "the seed is on another filesystem, so it can't be hardlinked; {kept}"
        ));
        return Ok(notes);
    }
    let grafted = graft_clone(&planting.tree, &seeded, &planting.source, entry, link_min)?;
    notes.push(format!("grafted from the seed: {grafted}"));
    record.set(&entry.path, seed.parts(&entry.path, entry.parts));
    Ok(notes)
}

/// Graft the cache `entry` declares straight from the main checkout at
/// `source` into `tree`, replacing any the tree has, as `cache doctor` does to
/// test what a build in the tree writes through to the main checkout.
fn graft_from_main(tree: &Path, source: &Path, entry: &Entry, link_min: u64) -> Result<String> {
    let src = source.join(&entry.path);
    check_replaceable(tree, &entry.path, true)?;
    if !src.is_dir() {
        return Ok("the main checkout has none".to_owned());
    }
    let parent = tree.join(&entry.path);
    let parent = parent.parent().unwrap_or(tree);
    fs::create_dir_all(parent).context(format!("could not create {}", parent.display()))?;
    if !clone::same_filesystem(&src, parent) {
        return Ok(format!(
            "{} is on another filesystem, so it can't be hardlinked",
            src.display()
        ));
    }
    let grafted = graft_clone(tree, &src, source, entry, link_min)?;
    Ok(format!("grafted from the main checkout: {grafted}"))
}

/// Clone the cache at `src` into `tree` at the path `entry` declares, in place
/// of whatever the tree has there, returning what it cost. `main` is the main
/// checkout, whose symlinks into it are counted.
fn graft_clone(
    tree: &Path,
    src: &Path,
    main: &Path,
    entry: &Entry,
    link_min: u64,
) -> Result<String> {
    let dst = tree.join(&entry.path);
    // Clone beside the cache and move it into place, so that an interrupted
    // graft never leaves a partial cache that looks whole.
    let staging = staging_path(&entry.path, "workforest-graft");
    discard_staging(tree, &staging)?;
    let staged = tree.join(&staging);
    let grafted = clone::clone_dir(src, &staged, main, link_min, &entry.always_copy)
        .context(format!("could not clone {}", src.display()))
        .and_then(|cloned| {
            // Before the graft is in place, so that one cut short never leaves
            // output that passes as built from the tree's files. A tree
            // planted just now is newer than anything grafted into it, but
            // one planted or edited before the cache's last build is not.
            let marked = match cloned.newest {
                Some(newest) => mark_older_changed(tree, newest, &staging).map_err(|err| {
                    Error::new(format!(
                        "{err}; dropped the graft, so that its output can't pass as fresh"
                    ))
                })?,
                None => 0,
            };
            if dst.symlink_metadata().is_ok() {
                remove(tree, &entry.path)?;
            }
            fs::rename(&staged, &dst).context(format!("could not move it to {}", dst.display()))?;
            Ok((cloned, marked))
        });
    if grafted.is_err() {
        let _ = fs::remove_dir_all(&staged);
    }
    let (cloned, marked) = grafted?;
    let mut note = format!(
        "{}, {} hardlinked, {} copied",
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
/// would track, in its submodules too, and that is no newer than `newest`,
/// returning how many. Build tools take output newer than its sources for
/// built from them, so output grafted from a build that came after the tree's
/// checkout or edits would otherwise pass as fresh, though built from another
/// checkout's sources. What is under `staged`, the graft on its way into
/// place, is left alone: its files are hardlinked to other copies.
fn mark_older_changed(tree: &Path, newest: SystemTime, staged: &str) -> Result<u64> {
    let Some(files) = git::files(tree) else {
        bail!("could not list the files of {}", tree.display());
    };
    let now = FileTimes::new().set_modified(SystemTime::now());
    let mut marked = 0;
    for file in files {
        if file.starts_with(staged) {
            continue;
        }
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

/// Where the cache at `path` is staged for `purpose`, such as being cloned
/// before it is moved into place: beside it, so on the same filesystem, under
/// a name that is workforest's own.
fn staging_path(path: &str, purpose: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/.{name}.{purpose}"),
        None => format!(".{path}.{purpose}"),
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
/// is still hardlinked to another copy, and how its newest file's age compares
/// with the seed at `seed`'s, or why there's nothing to count.
pub fn describe(tree: &Path, seed: &Path, entry: &Entry) -> String {
    match state(tree, entry) {
        CacheState::Never => "never".to_owned(),
        CacheState::Cold => "cold".to_owned(),
        CacheState::Blocked(problem) => problem,
        CacheState::Grafted(usage) => {
            let mut shown = format!(
                "{}, {} hardlinked, {} own",
                count(usage.files, "file"),
                human_bytes(usage.shared),
                human_bytes(usage.own)
            );
            if let Some(ours) = usage.newest {
                let seeded = match newest(&seed.join(&entry.path)) {
                    Some(seeded) => format!("the seed's: {} old", age(seeded)),
                    None => "the seed has none".to_owned(),
                };
                shown.push_str(&format!(", newest {} old ({seeded})", age(ours)));
            }
            shown
        }
    }
}

/// How many files a copy of a cache holds, how many bytes, and how old its
/// newest file is.
pub fn describe_usage(usage: &clone::Usage) -> String {
    let mut shown = format!(
        "{}, {}",
        count(usage.files, "file"),
        human_bytes(usage.shared + usage.own)
    );
    if let Some(newest) = usage.newest {
        shown.push_str(&format!(", newest {} old", age(newest)));
    }
    shown
}

/// The modification time of the newest file in the cache at `dir`, if it
/// holds any. Unreadable parts are left out.
pub fn newest(dir: &Path) -> Option<SystemTime> {
    walk::walk(dir)
        .flatten()
        .filter(|(_, meta)| meta.is_file())
        .filter_map(|(_, meta)| meta.modified().ok())
        .max()
}

/// Where a tree's cache stands.
pub enum CacheState {
    /// Its mode says to leave it cold.
    Never,
    /// It has none.
    Cold,
    /// workforest won't touch it, for the reason given.
    Blocked(String),
    /// It has one, sharing this much with other copies.
    Grafted(clone::Usage),
}

pub fn state(tree: &Path, entry: &Entry) -> CacheState {
    let path = tree.join(&entry.path);
    match entry.mode {
        Mode::Never => CacheState::Never,
        Mode::Clone if path.is_symlink() || !path.is_dir() => CacheState::Cold,
        Mode::Clone => match check_replaceable(tree, &entry.path, true) {
            Err(err) => CacheState::Blocked(err.to_string()),
            Ok(()) => CacheState::Grafted(clone::usage(&path)),
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

/// How long ago `time` was, roughly, in the largest unit that keeps it at 1 or
/// more: `12 s`, `5 min`, `3 h`, `58 days`. A time in the future is `0 s`.
fn age(time: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(time)
        .map_or(0, |ago| ago.as_secs());
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        3600..86400 => format!("{} h", secs / 3600),
        _ => count(secs / 86400, "day"),
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
