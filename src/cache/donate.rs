//! Donating a tree's build cache back to its main checkout, so that trees
//! planted later graft a warm one.
//!
//! Grafting only goes one way, so when work only happens in forests, the main
//! checkout's caches only get older, and every new tree starts from them. A
//! donation puts a tree's cache in place of the main checkout's. A tree about
//! to be removed gives its cache up: it is moved, which costs no disk. A tree
//! that lives on keeps its own, and the main checkout gets a clone of it, made
//! as a graft is, with large files hardlinked and small ones copied.
//!
//! A donation replaces the main checkout's cache rather than merging into it:
//! the tree's was grafted from the main checkout's and built on, so it is
//! whole on its own. It only goes ahead when it helps: when the tree has
//! output newer than the main checkout's, and the main checkout has none newer
//! than the tree's that it would lose. A cache whose top-level directories are
//! caches of their own, as Cargo's profiles and target triples are, gives up
//! just the directories that are newer. A main checkout that has built since
//! its HEAD moved to commits the tree lacks keeps its cache too, which may be
//! for newer code. `--force` donates whatever the ages.
//!
//! The donated output was built from the tree's sources, which may differ from
//! the main checkout's. Build tools take output newer than its sources for
//! built from them, so the main checkout's files older than the donated output
//! are marked as changed, as a graft marks a tree's, and the main checkout
//! rebuilds its own code once.
//!
//! A running build holds its cache's lock files, as Cargo holds
//! `debug/.cargo-lock`. A donation never takes a cache that a build in the
//! tree may still be writing, nor replaces one that a build in the main
//! checkout is using, and it holds the locks it takes until it is done, so
//! that no build starts halfway through.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, TryLockError};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::walk::walk;
use super::{
    Entry, Mode, age, check_replaceable, clone, count, declared, discard_staging, human_bytes,
    mark_older_changed, staging_path, warn,
};
use crate::error::{Context, Error, Result, bail};
use crate::git;

/// How a donated cache gets to the main checkout.
#[derive(Clone, Copy)]
pub enum Transfer {
    /// Moved, from a tree about to be removed, which is left without it.
    Move,
    /// Cloned, from a tree that lives on: files of `link_min` bytes and up
    /// hardlinked, smaller ones and those matching an always-copy glob copied.
    Clone { link_min: u64 },
}

/// Donate each cache that the repo with its main worktree at `source` declares
/// from `tree` to the main checkout, printing a line for each, after
/// `header` if one is given and there is anything to say. `force` donates
/// whatever the caches' ages. A cache that can't be donated is left where it
/// was, with a warning.
pub fn donate_tree(
    tree: &Path,
    source: &Path,
    transfer: Transfer,
    force: bool,
    header: Option<&str>,
) {
    let mut header = header;
    let mut announce = || {
        if let Some(header) = header.take() {
            println!("{header}");
        }
    };
    let declared = declared(source);
    warn(&declared.warnings);
    for entry in &declared.entries {
        match donate(tree, source, entry, transfer, force) {
            Ok(Some(note)) => {
                announce();
                println!("  cache {}: {note}", entry.path);
            }
            Ok(None) => {}
            Err(err) => {
                announce();
                eprintln!("workforest: cache {}: {err}", entry.path);
            }
        }
    }
}

/// Donate one cache from `tree`, returning what happened, if anything.
fn donate(
    tree: &Path,
    source: &Path,
    entry: &Entry,
    transfer: Transfer,
    force: bool,
) -> Result<Option<String>> {
    if entry.mode == Mode::Never {
        return Ok(None);
    }
    let (from, to) = (tree.join(&entry.path), source.join(&entry.path));
    if from.is_symlink() || !from.is_dir() {
        // As in a graft, the many repos of a build system that keep no cache
        // of its pass quietly.
        return Ok((!entry.built_in).then(|| "the tree has none".to_owned()));
    }
    check_replaceable(tree, &entry.path, true)?;
    check_replaceable(source, &entry.path, true)
        .map_err(|err| Error::new(format!("in the main checkout, {err}")))?;
    if to.symlink_metadata().is_ok_and(|meta| !meta.is_dir()) {
        bail!(
            "the main checkout's {} isn't a directory, so it isn't replaced",
            entry.path
        );
    }
    let ancestor = to.ancestors().skip(1).find(|dir| dir.is_dir());
    let movable = ancestor.is_some_and(|dir| clone::same_filesystem(&from, dir))
        && (!to.is_dir() || clone::same_filesystem(&from, &to));
    if !movable {
        return Ok(Some(
            "the main checkout is on another filesystem, so it can't take the tree's; left alone"
                .to_owned(),
        ));
    }

    let ours = survey(&from, tree);
    let theirs = survey(&to, tree);
    let mut locks = Locks::default();
    if let Err(lock) = locks.take(&from, &ours.locks) {
        return Ok(Some(format!(
            "a build holds {} in the tree, so its cache may be half written; left alone",
            lock.display()
        )));
    }
    if let Err(lock) = locks.take(&to, &theirs.locks) {
        return Ok(Some(format!(
            "a build holds {} in the main checkout; left alone",
            lock.display()
        )));
    }
    let plan = match plan(&ours, &theirs, entry.parts, force) {
        Ok(plan) => plan,
        Err(why) => return Ok(Some(why)),
    };
    if !force && let Some(why) = behind_main(tree, source, theirs.newest(None)) {
        return Ok(Some(format!("{why}; --force donates anyway")));
    }

    // The cache paths, relative to the checkouts, that the donation replaces.
    let replaced: Vec<String> = match &plan.parts {
        None => vec![entry.path.clone()],
        Some(parts) => parts
            .iter()
            .map(|part| format!("{}/{part}", entry.path))
            .collect(),
    };
    let mut cloned = clone::Cloned::default();
    let (mut done, mut failed) = (0, None);
    for rel in &replaced {
        match put(tree, source, rel, transfer, &entry.always_copy) {
            Ok(done_cloning) => {
                cloned.add(done_cloning.unwrap_or_default());
                done += 1;
            }
            Err(err) => {
                failed = Some(err);
                break;
            }
        }
    }
    if done == 0
        && let Some(err) = failed.take()
    {
        return Err(err);
    }
    // Parts are caches of their own, so those given up before one failed stay.
    let replaced = &replaced[..done];
    let parts = plan.parts.as_ref().map(|parts| parts[..done].to_vec());
    let given = parts.as_deref();

    let newest = ours.newest(given);
    // The donated output was built from the tree's sources, not the main
    // checkout's, so it mustn't pass as built from the main checkout's.
    let marked = match newest.map(|newest| mark_older_changed(source, newest)) {
        None => 0,
        Some(Ok(marked)) => marked,
        Some(Err(err)) => {
            for rel in replaced {
                let _ = fs::remove_dir_all(source.join(rel));
            }
            bail!(
                "{err}; dropped the donation, so that its output can't pass as built from the \
                 main checkout's sources"
            );
        }
    };
    if let Some(err) = failed {
        bail!("{err}; donated {} before it", replaced.join(", "));
    }

    let mut note = match (&parts, transfer) {
        (None, Transfer::Move) => "moved to the main checkout".to_owned(),
        (None, Transfer::Clone { .. }) => "cloned to the main checkout".to_owned(),
        (Some(parts), Transfer::Move) => format!("moved {} to the main checkout", parts.join(", ")),
        (Some(parts), Transfer::Clone { .. }) => {
            format!("cloned {} to the main checkout", parts.join(", "))
        }
    };
    let (files, bytes) = ours.size(given);
    match transfer {
        Transfer::Move => {
            note.push_str(&format!(
                ": {}, {}",
                count(files, "file"),
                human_bytes(bytes)
            ));
        }
        Transfer::Clone { .. } => note.push_str(&format!(
            ": {}, {} hardlinked, {} copied",
            count(cloned.files, "file"),
            human_bytes(cloned.linked),
            human_bytes(cloned.copied)
        )),
    }
    if let Some(newest) = newest {
        note.push_str(&format!(", newest {} old", age(newest)));
    }
    match theirs.newest(given) {
        Some(replaced) => note.push_str(&format!(", replacing one {} old", age(replaced))),
        None => note.push_str("; the main checkout had none"),
    }
    if !plan.kept.is_empty() {
        note.push_str(&format!(
            "; kept the main checkout's newer {}",
            plan.kept.join(", ")
        ));
    }
    if marked > 0 {
        note.push_str(&format!(
            "; marked the main checkout's {} older than it as changed",
            count(marked, "file")
        ));
    }
    let links = ours.links_into(given);
    if links > 0 {
        let risk = match transfer {
            Transfer::Move => "which break once it is removed",
            Transfer::Clone { .. } => "so a build that writes through one changes it",
        };
        note.push_str(&format!(
            "; {} into the tree, {risk}",
            count(links, "symlink")
        ));
    }
    Ok(Some(note))
}

/// What a donation replaces of the main checkout's cache.
struct Plan {
    /// The top-level directories given up, or `None` for the whole cache.
    parts: Option<Vec<String>>,
    /// The main checkout's newer top-level directories, which it keeps.
    kept: Vec<String>,
}

/// What to donate, given what the tree's cache (`ours`) and the main
/// checkout's (`theirs`) hold, or why to leave them as they are. `by_parts`
/// says whether the cache's top-level directories are caches of their own.
fn plan(
    ours: &Survey,
    theirs: &Survey,
    by_parts: bool,
    force: bool,
) -> std::result::Result<Plan, String> {
    let whole = Plan {
        parts: None,
        kept: Vec::new(),
    };
    if force {
        return Ok(whole);
    }
    let ours_newer = ours.newer_than(theirs);
    let theirs_newer = theirs.newer_than(ours);
    if ours_newer.is_empty() {
        return Err("nothing in it is newer than the main checkout's; left alone".to_owned());
    }
    if theirs_newer.is_empty() {
        return Ok(whole);
    }
    let dirs = |survey: &Survey, names: &[&str]| -> Vec<String> {
        names
            .iter()
            .filter(|&&name| survey.parts[name].dir)
            .map(|&name| name.to_owned())
            .collect()
    };
    if by_parts {
        // A part only the tree has as a directory replaces nothing of the
        // main checkout's but a stray file.
        let given: Vec<String> = dirs(ours, &ours_newer)
            .into_iter()
            .filter(|name| theirs.parts.get(name).is_none_or(|part| part.dir))
            .collect();
        if !given.is_empty() {
            return Ok(Plan {
                parts: Some(given),
                kept: dirs(theirs, &theirs_newer),
            });
        }
    }
    let (are, them) = if theirs_newer.len() == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    Err(format!(
        "the main checkout's {} {are} newer than the tree's, so donating would lose {them}; \
         --force donates anyway",
        theirs_newer.join(", ")
    ))
}

/// Why the tree's code may be older than what the main checkout's cache,
/// whose newest file is from `main_newest`, was built from, if it may: the
/// main checkout's HEAD has commits that the tree's lacks, and its cache has
/// output from after HEAD last moved. Output from before then was built from
/// older code than its HEAD, so a main checkout that is pulled but never built
/// doesn't stop a donation.
fn behind_main(tree: &Path, source: &Path, main_newest: Option<SystemTime>) -> Option<String> {
    let main_newest = main_newest?;
    let head = git::output(
        source,
        ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    )?;
    let lacking = match git::count(tree, &format!("HEAD..{head}")) {
        Some(0) => return None,
        Some(lacking) => lacking,
        None => {
            return Some("could not compare the tree's HEAD with the main checkout's".to_owned());
        }
    };
    if git::head_moved(source).is_some_and(|moved| main_newest < moved) {
        return None;
    }
    Some(format!(
        "the main checkout's HEAD has {} the tree lacks, and it has built since moving there, \
         so its cache may be for newer code",
        count(lacking, "commit")
    ))
}

/// Put the tree's cache at `rel` into the main checkout at `source`, in place
/// of whatever the main checkout has there, returning what a clone cost.
fn put(
    tree: &Path,
    source: &Path,
    rel: &str,
    transfer: Transfer,
    always_copy: &[String],
) -> Result<Option<clone::Cloned>> {
    let (from, to) = (tree.join(rel), source.join(rel));
    let parent = to.parent().unwrap_or(source);
    fs::create_dir_all(parent).context(format!("could not create {}", parent.display()))?;
    let incoming = staging_path(rel, "workforest-donation");
    let outgoing = staging_path(rel, "workforest-old");
    // What an interrupted donation left behind.
    discard_staging(source, &incoming)?;
    discard_staging(source, &outgoing)?;
    let (incoming, outgoing) = (source.join(incoming), source.join(outgoing));

    // A clone is made beside the main checkout's cache, so that one cut short
    // never leaves a partial cache that looks whole.
    let (arriving, cloned) = match transfer {
        Transfer::Move => (from, None),
        Transfer::Clone { link_min } => {
            match clone::clone_dir(&from, &incoming, tree, link_min, always_copy) {
                Ok(cloned) => (incoming.clone(), Some(cloned)),
                Err(err) => {
                    let _ = fs::remove_dir_all(&incoming);
                    bail!("could not clone {}: {err}", from.display());
                }
            }
        }
    };
    let replacing = to.symlink_metadata().is_ok();
    let swapped = swap(&arriving, &to, &outgoing, replacing);
    if swapped.is_err() && cloned.is_some() {
        let _ = fs::remove_dir_all(&incoming);
    }
    swapped?;
    // Hardlinks that the old cache shared with trees keep their files alive.
    if replacing && let Err(err) = fs::remove_dir_all(&outgoing) {
        eprintln!(
            "workforest: could not remove the main checkout's old cache, {}: {err}",
            outgoing.display()
        );
    }
    Ok(cloned)
}

/// Move `arriving` to `to`, first moving what is there, if `replacing`, aside
/// to `outgoing`, and back again if `arriving` can't follow.
fn swap(arriving: &Path, to: &Path, outgoing: &Path, replacing: bool) -> Result<()> {
    if replacing {
        fs::rename(to, outgoing).context(format!("could not move {} aside", to.display()))?;
    }
    let moved = fs::rename(arriving, to);
    if moved.is_err() && replacing {
        let _ = fs::rename(outgoing, to);
    }
    moved.context(format!("could not move it to {}", to.display()))
}

/// What a walk of one checkout's copy of a cache found.
#[derive(Default)]
struct Survey {
    /// Each top-level directory and regular file, by name. Names that aren't
    /// UTF-8 are left out, so they only move with the whole cache.
    parts: BTreeMap<String, Part>,
    /// The files that look like locks, relative to the cache.
    locks: Vec<PathBuf>,
}

/// What a top-level directory or file of a cache holds.
#[derive(Default)]
struct Part {
    dir: bool,
    /// Regular files, a top-level file counting itself.
    files: u64,
    bytes: u64,
    /// The latest modification time of one of its files. Directories don't
    /// count: a graft creates them, so theirs are no sign of a build.
    newest: Option<SystemTime>,
    /// Symlinks whose absolute target is in the tree.
    links_into: u64,
}

/// Walk the cache at `dir`, which is missing if the checkout has none, noting
/// the symlinks that point into `tree`. Unreadable parts are left out.
fn survey(dir: &Path, tree: &Path) -> Survey {
    let mut survey = Survey::default();
    for (rel, meta) in walk(dir).flatten() {
        let kind = meta.file_type();
        if kind.is_file() && is_lock(&rel) {
            survey.locks.push(rel.clone());
        }
        let mut components = rel.components();
        let Some(top) = components.next().and_then(|top| top.as_os_str().to_str()) else {
            continue;
        };
        // A walk yields every top-level entry before what they hold.
        if components.next().is_none() && (kind.is_dir() || kind.is_file()) {
            let part = Part {
                dir: kind.is_dir(),
                ..Part::default()
            };
            survey.parts.insert(top.to_owned(), part);
        }
        let Some(part) = survey.parts.get_mut(top) else {
            continue;
        };
        if kind.is_file() {
            part.files += 1;
            part.bytes += meta.len();
            part.newest = part.newest.max(meta.modified().ok());
        } else if kind.is_symlink()
            && fs::read_link(dir.join(&rel))
                .is_ok_and(|target| target.is_absolute() && target.starts_with(tree))
        {
            part.links_into += 1;
        }
    }
    survey
}

impl Survey {
    /// The names of the parts holding a file newer than any in the same part
    /// of `other`, which counts as older for a part it doesn't have.
    fn newer_than(&self, other: &Survey) -> Vec<&str> {
        self.parts
            .iter()
            .filter(|(name, part)| {
                let theirs = other.parts.get(*name).and_then(|part| part.newest);
                part.newest.is_some() && theirs < part.newest
            })
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// The parts `names` names, or all of them.
    fn some<'a>(&'a self, names: Option<&'a [String]>) -> impl Iterator<Item = &'a Part> {
        self.parts
            .iter()
            .filter(move |(name, _)| names.is_none_or(|names| names.contains(name)))
            .map(|(_, part)| part)
    }

    fn newest(&self, names: Option<&[String]>) -> Option<SystemTime> {
        self.some(names).filter_map(|part| part.newest).max()
    }

    /// How many files, and how many bytes, the parts `names` names hold.
    fn size(&self, names: Option<&[String]>) -> (u64, u64) {
        self.some(names).fold((0, 0), |(files, bytes), part| {
            (files + part.files, bytes + part.bytes)
        })
    }

    fn links_into(&self, names: Option<&[String]>) -> u64 {
        self.some(names).map(|part| part.links_into).sum()
    }
}

/// Whether the file at `rel` looks like a lock: `.cargo-lock`, rustc's
/// `s-….lock`, LevelDB's `LOCK`.
fn is_lock(rel: &Path) -> bool {
    rel.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.ends_with(".lock") || name.ends_with("-lock") || name.eq_ignore_ascii_case("lock")
        })
}

/// The most lock files a donation holds at once, to stay clear of the limit
/// on open files. A build tool takes its build-wide lock near the top of its
/// cache, as Cargo takes `debug/.cargo-lock`, so the shallowest are held, and
/// the rest are only checked: rustc keeps one beside each incremental session.
const HELD_MAX: usize = 64;

/// The lock files a donation holds until it is done.
#[derive(Default)]
struct Locks {
    held: Vec<File>,
    /// Each held file's device and inode, so that one hardlinked into both
    /// checkouts isn't taken for held by a build.
    ids: HashSet<(u64, u64)>,
}

impl Locks {
    /// Take each of `locks`, relative to `dir`, shallowest first, failing with
    /// the first that something else holds. One that can't be opened or
    /// locked at all, as on a filesystem without locks, can't say.
    fn take(&mut self, dir: &Path, locks: &[PathBuf]) -> std::result::Result<(), PathBuf> {
        let mut locks: Vec<&PathBuf> = locks.iter().collect();
        locks.sort_by_key(|rel| rel.components().count());
        for rel in locks {
            let Ok(file) = File::open(dir.join(rel)) else {
                continue;
            };
            let Ok(meta) = file.metadata() else {
                continue;
            };
            if self.ids.contains(&(meta.dev(), meta.ino())) {
                continue;
            }
            match file.try_lock() {
                Ok(()) if self.held.len() < HELD_MAX => {
                    self.ids.insert((meta.dev(), meta.ino()));
                    self.held.push(file);
                }
                // Closing the file lets the lock go.
                Ok(()) | Err(TryLockError::Error(_)) => {}
                Err(TryLockError::WouldBlock) => return Err(rel.clone()),
            }
        }
        Ok(())
    }
}
