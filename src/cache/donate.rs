//! Giving caches to a repo's seed (see [`super::seed`]): a tree's, donated so
//! that trees planted later graft a warm one, and the main checkout's, which
//! the seed takes whenever they have newer output.
//!
//! A tree about to be removed gives its cache up: it is moved, which costs no
//! disk. A tree that lives on keeps its own, and the seed gets a clone of it,
//! made as a graft is, with large files hardlinked and small ones copied. The
//! main checkout's are always cloned, so the main checkout is only ever read.
//!
//! What is given replaces the seed's rather than merging into it: a tree's
//! cache was grafted from the seed and built on, so it is whole on its own. A
//! cache whose top-level directories are caches of their own, as Cargo's
//! profiles and target triples are, gives up just the directories that are
//! newer. A tree gives only what is newer than the seed's, and only in place of
//! parts still as it grafted them: one changed since, by another tree or the
//! main checkout, holds work the tree's lacks. `--force` donates the whole
//! cache whatever the ages.
//!
//! Nothing a donation does needs the main checkout's sources marked as
//! changed, as nothing is built in the seed: a graft marks a tree's files older
//! than what it grafts (see [`super::mark_older_changed`]).
//!
//! A running build holds its cache's lock files, as Cargo holds
//! `debug/.cargo-lock`. A cache is never taken while a build may still be
//! writing it, as far as lock files can tell, and the locks are held until it
//! is done, so that no build starts halfway through. A build tool that holds no
//! lock file in its cache can't be seen.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, TryLockError};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::seed::{Record, STAGING, Seed, WHOLE};
use super::walk::walk;
use super::{
    Entry, Mode, Planting, age, check_replaceable, clone, count, declared, human_bytes, open_seed,
    warn,
};
use crate::error::{Context, Result, bail};

/// How a donated cache gets to the seed.
#[derive(Clone, Copy)]
pub enum Transfer {
    /// Moved, from a tree about to be removed, which is left without it.
    Move,
    /// Cloned, from a tree that lives on: files of `link_min` bytes and up
    /// hardlinked, smaller ones and those matching an always-copy glob copied.
    Clone { link_min: u64 },
}

/// What a donation reports.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Say {
    /// What happened to each cache, as `cache donate` reports it.
    All,
    /// What was donated, and why a cache that could have been wasn't, as
    /// removing a tree reports it: not the caches with nothing to give.
    Notable,
    /// Nothing but errors, as for JSON.
    Nothing,
}

/// Donate each cache that the tree's repo declares to the repo's seed, printing
/// a line for each as `say` says, after `header` if one is given and there is
/// anything to say. `force` donates whatever the caches' ages. A cache that
/// can't be donated is left where it was, with a warning.
pub fn donate_tree(
    planting: &Planting,
    transfer: Transfer,
    force: bool,
    say: Say,
    header: Option<&str>,
) {
    let mut header = header;
    let mut announce = || {
        if let Some(header) = header.take() {
            println!("{header}");
        }
    };
    let declared = declared(&planting.source);
    warn(&declared.warnings);
    let mut record = Record::read(&planting.record);
    let mut seed = None;
    for entry in &declared.entries {
        match donate(planting, &mut seed, &mut record, entry, transfer, force) {
            Ok(Some(note)) => {
                if say == Say::All || (say == Say::Notable && note.notable) {
                    announce();
                    println!("  cache {}: {}", entry.path, note.text);
                }
            }
            Ok(None) => {}
            Err(err) => {
                if say != Say::Nothing {
                    announce();
                }
                eprintln!("workforest: cache {}: {err}", entry.path);
            }
        }
    }
    // A tree that keeps its cache now shares what it gave with the seed.
    if let Transfer::Clone { .. } = transfer
        && let Err(err) = record.write()
    {
        eprintln!("workforest: {err}");
    }
}

/// What a donation says about one cache.
struct Note {
    text: String,
    /// Whether it is worth saying when a tree is removed.
    notable: bool,
}

impl Note {
    fn notable(text: impl Into<String>) -> Note {
        Note {
            text: text.into(),
            notable: true,
        }
    }

    fn quiet(text: impl Into<String>) -> Note {
        Note {
            text: text.into(),
            notable: false,
        }
    }
}

/// Donate one cache from the tree, returning what happened, if anything.
fn donate(
    planting: &Planting,
    seed: &mut Option<Seed>,
    record: &mut Record,
    entry: &Entry,
    transfer: Transfer,
    force: bool,
) -> Result<Option<Note>> {
    if entry.mode == Mode::Never {
        return Ok(None);
    }
    let from = planting.tree.join(&entry.path);
    if from.is_symlink() || !from.is_dir() {
        // As in a graft, the many repos of a build system that keep no cache
        // of its pass quietly.
        return Ok((!entry.built_in).then(|| Note::quiet("the tree has none")));
    }
    check_replaceable(&planting.tree, &entry.path, true)?;
    let seed = open_seed(seed, planting)?;
    let grafted = record.grafted(&entry.path).cloned();
    let giver = Giver {
        dir: &from,
        root: &planting.tree,
        grafted: grafted.as_ref(),
    };
    let outcome = give(seed, giver, entry, transfer, force)?;
    let given = match outcome.given {
        Ok(given) => given,
        Err(Refusal::OtherFilesystem) => {
            return Ok(Some(Note::notable(
                "the seed is on another filesystem, so it can't take the tree's; left alone",
            )));
        }
        Err(Refusal::Locked(lock)) => {
            return Ok(Some(Note::notable(format!(
                "a build holds {} in the tree, so its cache may be half written; left alone",
                lock.display()
            ))));
        }
        Err(Refusal::NothingNewer) => {
            return Ok(Some(Note::quiet(
                "nothing in it is newer than the seed's; left alone",
            )));
        }
        Err(Refusal::Changed(parts)) => {
            let (what, them) = changed(&parts);
            return Ok(Some(Note::notable(format!(
                "{what} changed since this tree grafted {them}, so donating would lose that; \
                 --force donates anyway"
            ))));
        }
    };
    if let Transfer::Clone { .. } = transfer {
        for part in &given.renewed {
            record.set_part(&entry.path, part, given.generation);
        }
    }

    let mut note = match (&given.parts, transfer) {
        (None, Transfer::Move) => "moved to the seed".to_owned(),
        (None, Transfer::Clone { .. }) => "cloned to the seed".to_owned(),
        (Some(parts), Transfer::Move) => format!("moved {} to the seed", parts.join(", ")),
        (Some(parts), Transfer::Clone { .. }) => {
            format!("cloned {} to the seed", parts.join(", "))
        }
    };
    note.push_str(&format!(": {}", given.size(transfer)));
    if let Some(newest) = given.newest {
        note.push_str(&format!(", newest {} old", age(newest)));
    }
    match given.replaced {
        Some(replaced) => note.push_str(&format!(", replacing one {} old", age(replaced))),
        None => note.push_str("; the seed had none"),
    }
    if !outcome.kept.is_empty() {
        let (what, them) = changed(&outcome.kept);
        note.push_str(&format!(
            "; kept {what}, changed since this tree grafted {them}"
        ));
    }
    if given.links_into > 0 {
        let risk = match transfer {
            Transfer::Move => "which break once it is removed",
            Transfer::Clone { .. } => "so a build that writes through one changes it",
        };
        note.push_str(&format!(
            "; {} into the tree, {risk}",
            count(given.links_into, "symlink")
        ));
    }
    Ok(Some(Note::notable(note)))
}

/// `the seed's debug, release` and `them`, or `the seed's` and `it` for the
/// whole cache.
fn changed(parts: &[String]) -> (String, &'static str) {
    match parts {
        [] => ("the seed's".to_owned(), "it"),
        [part] => (format!("the seed's {part}"), "it"),
        parts => (format!("the seed's {}", parts.join(", ")), "them"),
    }
}

/// Have the seed take the main checkout at `source`'s copy of the cache
/// `entry` declares where it has newer output, returning what the seed took,
/// or why it took nothing, if that is worth saying.
pub fn take_from_main(
    seed: &mut Seed,
    source: &Path,
    entry: &Entry,
    link_min: u64,
) -> Result<Option<String>> {
    let from = source.join(&entry.path);
    let had = seed.dir.join(&entry.path).is_dir();
    let giver = Giver {
        dir: &from,
        root: source,
        grafted: None,
    };
    let outcome = give(seed, giver, entry, Transfer::Clone { link_min }, false)?;
    let given = match outcome.given {
        Ok(given) => given,
        Err(Refusal::OtherFilesystem) => {
            return Ok(Some(
                "the main checkout's is on another filesystem from the seed, so the seed can't \
                 take it"
                    .to_owned(),
            ));
        }
        Err(Refusal::Locked(lock)) => {
            return Ok(Some(format!(
                "a build holds {} in the main checkout, so the seed keeps what it has",
                lock.display()
            )));
        }
        Err(Refusal::NothingNewer | Refusal::Changed(_)) => return Ok(None),
    };
    let what = match (&given.parts, had) {
        (_, false) => "seeded from the main checkout".to_owned(),
        (None, true) => "the seed took the main checkout's newer cache".to_owned(),
        (Some(parts), true) => {
            format!(
                "the seed took the main checkout's newer {}",
                parts.join(", ")
            )
        }
    };
    Ok(Some(format!(
        "{what}: {}",
        given.size(Transfer::Clone { link_min })
    )))
}

/// Who gives a cache to the seed.
struct Giver<'a> {
    /// Its copy of the cache.
    dir: &'a Path,
    /// Its checkout, so that symlinks into it can be counted.
    root: &'a Path,
    /// For a tree that grafted the cache from the seed, the generation of each
    /// part it grafted. `None` takes newer output as better, whatever changed
    /// in the seed meanwhile.
    grafted: Option<&'a BTreeMap<String, u64>>,
}

/// What came of giving one cache to the seed.
struct Outcome {
    given: std::result::Result<Given, Refusal>,
    /// The parts with newer output that the seed kept, as they changed since
    /// the giver grafted them.
    kept: Vec<String>,
}

/// Why nothing was given.
enum Refusal {
    OtherFilesystem,
    /// A build holds this lock file, relative to the cache.
    Locked(PathBuf),
    NothingNewer,
    /// Each part with newer output changed in the seed since the giver grafted
    /// it: these, or the whole cache if none are named.
    Changed(Vec<String>),
}

/// What was given.
struct Given {
    /// The top-level directories given, or `None` for the whole cache.
    parts: Option<Vec<String>>,
    /// For a move, its files and bytes; for a clone, what cloning cost.
    files: u64,
    bytes: u64,
    cloned: clone::Cloned,
    /// The newest file given, and the newest of what it replaced.
    newest: Option<SystemTime>,
    replaced: Option<SystemTime>,
    links_into: u64,
    /// The seed's parts renewed, and their new generation.
    renewed: Vec<String>,
    generation: u64,
}

impl Given {
    fn size(&self, transfer: Transfer) -> String {
        match transfer {
            Transfer::Move => format!("{}, {}", count(self.files, "file"), human_bytes(self.bytes)),
            Transfer::Clone { .. } => format!(
                "{}, {} hardlinked, {} copied",
                count(self.cloned.files, "file"),
                human_bytes(self.cloned.linked),
                human_bytes(self.cloned.copied)
            ),
        }
    }
}

/// Put what `giver`'s copy of the cache `entry` declares has newer than the
/// seed's into the seed, in place of the seed's.
fn give(
    seed: &mut Seed,
    giver: Giver,
    entry: &Entry,
    transfer: Transfer,
    force: bool,
) -> Result<Outcome> {
    let refused = |why| {
        Ok(Outcome {
            given: Err(why),
            kept: Vec::new(),
        })
    };
    let to = seed.dir.join(&entry.path);
    let movable = clone::same_filesystem(giver.dir, &seed.dir)
        && (!to.is_dir() || clone::same_filesystem(giver.dir, &to));
    if !movable {
        return refused(Refusal::OtherFilesystem);
    }
    let ours = survey(giver.dir, giver.root);
    let theirs = survey(&to, giver.root);
    let mut locks = Locks::default();
    if let Err(lock) = locks.take(giver.dir, &ours.locks) {
        return refused(Refusal::Locked(lock));
    }
    let seed_has = to.is_dir();
    // Whether the seed's `part` is as the giver grafted it, if it can tell.
    let unchanged = |part: &str| {
        let present = if part == WHOLE {
            seed_has
        } else {
            theirs.parts.get(part).is_some_and(|part| part.dir)
        };
        match giver.grafted {
            Some(grafted) if present => {
                grafted.get(part) == Some(&seed.generation(&entry.path, part))
            }
            _ => true,
        }
    };
    let (parts, kept) = match plan(&ours, &theirs, seed_has, entry.parts, force, unchanged) {
        Ok(plan) => plan,
        Err(why) => return refused(why),
    };

    // What is given, and where it goes in the seed.
    let pieces: Vec<(PathBuf, String)> = match &parts {
        None => vec![(giver.dir.to_path_buf(), entry.path.clone())],
        Some(parts) => parts
            .iter()
            .map(|part| (giver.dir.join(part), format!("{}/{part}", entry.path)))
            .collect(),
    };
    let mut cloned = clone::Cloned::default();
    let (mut done, mut failed) = (0, None);
    for (from, rel) in &pieces {
        match put(
            &seed.dir,
            from,
            rel,
            giver.root,
            transfer,
            &entry.always_copy,
        ) {
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
    // Parts are caches of their own, so those given before one failed stay.
    let parts = parts.map(|parts| parts[..done].to_vec());
    let renewed: Vec<String> = match &parts {
        Some(parts) => parts.clone(),
        None if entry.parts => seed.parts(&entry.path, true).into_keys().collect(),
        None => vec![WHOLE.to_owned()],
    };
    let generation = seed.renew(&entry.path, &renewed, entry.parts)?;
    if let Some(err) = failed {
        let gave: Vec<&str> = pieces[..done].iter().map(|(_, rel)| rel.as_str()).collect();
        bail!("{err}; gave {} before it", gave.join(", "));
    }
    let given_names = parts.as_deref();
    let (files, bytes) = ours.size(given_names);
    Ok(Outcome {
        given: Ok(Given {
            files,
            bytes,
            cloned,
            newest: ours.newest(given_names),
            replaced: theirs.newest(given_names),
            links_into: ours.links_into(given_names),
            parts,
            renewed,
            generation,
        }),
        kept,
    })
}

/// What to give, given what the giver's copy of a cache (`ours`) and the
/// seed's (`theirs`) hold, or why to give nothing: the top-level directories
/// to give, or `None` for the whole cache, and the newer ones kept as they
/// aren't `unchanged` since the giver grafted them. `seed_has` says whether
/// the seed has the cache at all, and `by_parts` whether its top-level
/// directories are caches of their own.
fn plan(
    ours: &Survey,
    theirs: &Survey,
    seed_has: bool,
    by_parts: bool,
    force: bool,
    unchanged: impl Fn(&str) -> bool,
) -> std::result::Result<(Option<Vec<String>>, Vec<String>), Refusal> {
    if force {
        return Ok((None, Vec::new()));
    }
    if !seed_has {
        return match ours.newest(None) {
            Some(_) => Ok((None, Vec::new())),
            None => Err(Refusal::NothingNewer),
        };
    }
    if !by_parts {
        return if ours.newest(None) <= theirs.newest(None) {
            Err(Refusal::NothingNewer)
        } else if unchanged(WHOLE) {
            Ok((None, Vec::new()))
        } else {
            Err(Refusal::Changed(Vec::new()))
        };
    }
    // A part only the giver has as a directory replaces nothing of the seed's
    // but a stray file.
    let newer: Vec<String> = ours
        .newer_than(theirs)
        .into_iter()
        .filter(|&name| ours.parts[name].dir)
        .filter(|&name| theirs.parts.get(name).is_none_or(|part| part.dir))
        .map(str::to_owned)
        .collect();
    let (given, kept): (Vec<String>, Vec<String>) =
        newer.into_iter().partition(|name| unchanged(name));
    match (given.is_empty(), kept.is_empty()) {
        (false, _) => Ok((Some(given), kept)),
        (true, true) => Err(Refusal::NothingNewer),
        (true, false) => Err(Refusal::Changed(kept)),
    }
}

/// Put the cache at `from` into the seed at `seed`, at `rel`, in place of
/// whatever the seed has there, returning what a clone cost. `root` is the
/// checkout `from` is in.
fn put(
    seed: &Path,
    from: &Path,
    rel: &str,
    root: &Path,
    transfer: Transfer,
    always_copy: &[String],
) -> Result<Option<clone::Cloned>> {
    let to = seed.join(rel);
    let parent = to.parent().unwrap_or(seed);
    fs::create_dir_all(parent).context(format!("could not create {}", parent.display()))?;
    // Opening the seed cleared what an interrupted donation left here.
    let staging = seed.join(STAGING);
    fs::create_dir(&staging).context(format!("could not create {}", staging.display()))?;
    let (incoming, outgoing) = (staging.join("incoming"), staging.join("outgoing"));

    // A clone is made beside the seed's cache, so that one cut short never
    // leaves a partial cache that looks whole.
    let (arriving, cloned) = match transfer {
        Transfer::Move => (from.to_path_buf(), None),
        Transfer::Clone { link_min } => {
            match clone::clone_dir(from, &incoming, root, link_min, always_copy) {
                Ok(cloned) => (incoming.clone(), Some(cloned)),
                Err(err) => {
                    let _ = fs::remove_dir_all(&staging);
                    bail!("could not clone {}: {err}", from.display());
                }
            }
        }
    };
    let replacing = to.symlink_metadata().is_ok();
    let swapped = swap(&arriving, &to, &outgoing, replacing);
    // Hardlinks that the old cache shared with trees keep their files alive.
    if let Err(err) = fs::remove_dir_all(&staging) {
        eprintln!(
            "workforest: could not remove the seed's old cache, {}: {err}",
            staging.display()
        );
    }
    swapped?;
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

/// What a walk of one copy of a cache found.
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
    /// Symlinks whose absolute target is in the checkout.
    links_into: u64,
}

/// Walk the cache at `dir`, which is missing if there is none, noting the
/// symlinks that point into `root`. Unreadable parts are left out.
fn survey(dir: &Path, root: &Path) -> Survey {
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
                .is_ok_and(|target| target.is_absolute() && target.starts_with(root))
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
    /// Each held file's device and inode, so that one hardlinked twice isn't
    /// taken for held by a build.
    ids: HashSet<(u64, u64)>,
}

impl Locks {
    /// Take each of `locks`, relative to `dir`, shallowest first, then by
    /// path, failing with the first that something else holds. One that can't
    /// be opened or locked at all, as on a filesystem without locks, can't say.
    fn take(&mut self, dir: &Path, locks: &[PathBuf]) -> std::result::Result<(), PathBuf> {
        let mut locks: Vec<&PathBuf> = locks.iter().collect();
        locks.sort_by_key(|rel| (rel.components().count(), *rel));
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
