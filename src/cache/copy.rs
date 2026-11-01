//! The main checkout's ignored files, copied into a tree so that it works as
//! the main checkout does without being set up again: a `.env`,
//! `node_modules`, a downloaded tool. Each copy is the tree's own: nothing is
//! hardlinked, so changing it changes nothing else, and nothing is given back.
//!
//! What stays behind:
//!
//! - what git doesn't ignore, which is work in progress rather than setup;
//! - the caches a repo declares, which are grafted instead, or left cold;
//! - what lies outside a sparse tree's directories;
//! - what the tree's own branch doesn't ignore, which would show up in its
//!   status as untracked;
//! - what names the path it was made at, where a copy would install or build
//!   into the main checkout, or share its git state;
//! - what the tree has something at already: a copy never replaces anything.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use super::{Planting, clone, count, declared, human_bytes, mark_older_changed};
use crate::error::{Context, Result, bail};
use crate::git;

/// How many copies are listed one by one before the rest are summed up.
const LISTED: usize = 8;

/// One path being copied, staged beside where it goes until every copy is
/// ready to move into place.
struct Copy {
    rel: PathBuf,
    staged: PathBuf,
    cloned: clone::Cloned,
    kind: Kind,
}

enum Kind {
    File,
    Dir,
    Symlink,
}

/// Copy into the tree each path that git ignores in the main checkout and the
/// tree has nothing at yet, printing what was copied and what was left out.
/// One that can't be copied is left out with a warning: the tree works
/// without it.
pub fn copy_tree(planting: &Planting) {
    let (tree, source) = (&planting.tree, &planting.source);
    let Some(listed) = git::ignored(source) else {
        eprintln!(
            "workforest: could not list what git ignores in {}",
            source.display()
        );
        return;
    };
    let caches: Vec<PathBuf> = declared(source)
        .entries
        .into_iter()
        .map(|entry| PathBuf::from(entry.path))
        .collect();
    let sparse = git::sparse_dirs(tree);
    let mut left_out = BTreeMap::new();
    let mut wanted = Vec::new();
    for (rel, dir) in listed {
        let outside = sparse.as_ref().is_some_and(|dirs| !in_sparse(&rel, dirs));
        if outside || caches.iter().any(|cache| rel.starts_with(cache)) {
            continue;
        }
        if let Some((at, why)) = unmovable_around(source, &rel, dir) {
            left_out.insert(at, why.to_owned());
            continue;
        }
        if tree.join(&rel).symlink_metadata().is_err() {
            wanted.push((rel, dir));
        }
    }
    if wanted.is_empty() {
        report(&[], &left_out, 0);
        return;
    }
    let Some(ignored_here) = git::ignored_among(tree, &wanted) else {
        eprintln!(
            "workforest: could not tell what git ignores in {}, so copied nothing",
            tree.display()
        );
        return;
    };
    let mut staged = Vec::new();
    for (rel, _) in wanted {
        if !ignored_here.contains(&rel) {
            let why = "the tree's branch doesn't ignore it, so it would show up as untracked";
            left_out.insert(rel, why.to_owned());
            continue;
        }
        match stage(tree, source, &rel, &caches, &mut left_out) {
            Ok(Some(copy)) => staged.push(copy),
            Ok(None) => {}
            Err(err) => eprintln!("workforest: copy {}: {err}", rel.display()),
        }
    }
    // Before the copies are in place, as for a graft: one cut short must not
    // leave output that passes as built from the tree's older files.
    let mut marked = 0;
    if let Some(newest) = staged.iter().filter_map(|copy| copy.cloned.newest).max() {
        let staging: Vec<PathBuf> = staged.iter().map(|copy| copy.staged.clone()).collect();
        match mark_older_changed(tree, newest, &staging) {
            Ok(n) => marked = n,
            Err(err) => {
                eprintln!(
                    "workforest: {err}; dropped the copies, so that their output can't pass as fresh"
                );
                for copy in &staged {
                    let _ = discard(tree, &copy.staged);
                }
                return;
            }
        }
    }
    let mut copied = Vec::new();
    for copy in staged {
        let dst = tree.join(&copy.rel);
        match fs::rename(tree.join(&copy.staged), &dst) {
            Ok(()) => copied.push(copy),
            Err(err) => {
                eprintln!(
                    "workforest: copy {}: could not move it into place: {err}",
                    copy.rel.display()
                );
                let _ = discard(tree, &copy.staged);
            }
        }
    }
    report(&copied, &left_out, marked);
}

/// Copy `rel` from the main checkout at `source` to its staging path in
/// `tree`, leaving out the caches and unmovable directories inside it, which
/// it notes in `left_out`. Nothing for a socket, a FIFO or a device.
fn stage(
    tree: &Path,
    source: &Path,
    rel: &Path,
    caches: &[PathBuf],
    left_out: &mut BTreeMap<PathBuf, String>,
) -> Result<Option<Copy>> {
    let src = source.join(rel);
    let meta = src
        .symlink_metadata()
        .context(format!("could not read {}", src.display()))?;
    let staged = staging(rel);
    let at = tree.join(&staged);
    discard(tree, &staged)?;
    if let Some(parent) = at.parent() {
        fs::create_dir_all(parent).context(format!("could not create {}", parent.display()))?;
    }
    let file_type = meta.file_type();
    let mut cloned = clone::Cloned::default();
    let kind = if file_type.is_dir() {
        let inside = RefCell::new(Vec::new());
        let skip = |inner: &Path| {
            let rel = rel.join(inner);
            if caches.contains(&rel) {
                return true;
            }
            let why = unmovable(&source.join(&rel));
            if let Some(why) = why {
                inside.borrow_mut().push((rel, why));
            }
            why.is_some()
        };
        // Never hardlinked: the copy is the tree's to change.
        let copied = clone::clone_dir_skipping(&src, &at, source, u64::MAX, &[], Some(&skip));
        if copied.is_err() {
            let _ = fs::remove_dir_all(&at);
        }
        cloned = copied.context(format!("could not copy {}", src.display()))?;
        for (rel, why) in inside.into_inner() {
            left_out.insert(rel, why.to_owned());
        }
        if cloned.files == 0 {
            fs::remove_dir_all(&at).context(format!("could not remove {}", at.display()))?;
            return Ok(None);
        }
        Kind::Dir
    } else if file_type.is_file() {
        clone::copy_file(&src, &at, &meta).context(format!("could not copy {}", src.display()))?;
        cloned.files = 1;
        cloned.copied = meta.len();
        cloned.newest = meta.modified().ok();
        Kind::File
    } else if file_type.is_symlink() {
        let target = fs::read_link(&src).context(format!("could not read {}", src.display()))?;
        if target.is_absolute() && target.starts_with(source) {
            cloned.links_to_main = 1;
        }
        symlink(target, &at).context(format!("could not create {}", at.display()))?;
        cloned.files = 1;
        Kind::Symlink
    } else {
        return Ok(None);
    };
    Ok(Some(Copy {
        rel: rel.to_path_buf(),
        staged,
        cloned,
        kind,
    }))
}

/// Why the directory at `dir` doesn't work anywhere else, if it doesn't: it
/// names the path it was made at, so a copy would install or build into the
/// original.
fn unmovable(dir: &Path) -> Option<&'static str> {
    if dir.join("pyvenv.cfg").is_file() {
        return Some("a Python venv names the path it was made at");
    }
    if dir.join("CMakeCache.txt").is_file() {
        return Some("a CMake build directory names the source it was configured for");
    }
    if dir.file_name().is_some_and(|name| name == ".gradle") {
        return Some("Gradle's .gradle doesn't work at another path");
    }
    // A `.git` file, rather than a directory, names git state kept elsewhere.
    if dir.join(".git").is_file() {
        return Some("a git worktree or submodule names the repository it belongs to");
    }
    None
}

/// The unmovable directory of the main checkout at `source` that `rel`, a
/// directory if `dir`, is or is in, with why it is unmovable.
fn unmovable_around(source: &Path, rel: &Path, dir: bool) -> Option<(PathBuf, &'static str)> {
    let mut at = PathBuf::new();
    let parts: Vec<_> = rel.components().collect();
    for (n, part) in parts.iter().enumerate() {
        at.push(part);
        if n + 1 == parts.len() && !dir {
            break;
        }
        if let Some(why) = unmovable(&source.join(&at)) {
            return Some((at, why));
        }
    }
    None
}

/// Whether a sparse tree that checks out `dirs` has `rel` in it: at its top
/// level, as its top-level files are, or inside one of them.
fn in_sparse(rel: &Path, dirs: &[String]) -> bool {
    rel.components().count() == 1 || dirs.iter().any(|dir| rel.starts_with(dir))
}

/// Where `rel` is staged while it is copied: beside it, under a name that is
/// workforest's own.
fn staging(rel: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(rel.file_name().unwrap_or_default());
    name.push(".workforest-copy");
    rel.with_file_name(name)
}

/// Delete what an interrupted copy left at `staged` in `tree`. The name is
/// workforest's own, so only files git tracks there stop it.
fn discard(tree: &Path, staged: &Path) -> Result<()> {
    let at = tree.join(staged);
    let Ok(meta) = at.symlink_metadata() else {
        return Ok(());
    };
    if staged
        .to_str()
        .is_none_or(|staged| git::tracks(tree, staged))
    {
        bail!(
            "git tracks files under {}, so it can't stage a copy there",
            staged.display()
        );
    }
    let removed = if meta.is_dir() {
        fs::remove_dir_all(&at)
    } else {
        fs::remove_file(&at)
    };
    removed.context(format!("could not remove {}", at.display()))
}

/// Print a line for each path copied, up to a few, then one for the rest, and
/// one for each path left out.
fn report(copied: &[Copy], left_out: &BTreeMap<PathBuf, String>, marked: u64) {
    for copy in copied.iter().take(LISTED) {
        println!("  copy {}: {}", copy.rel.display(), describe(copy));
    }
    if let Some(rest) = copied.get(LISTED..).filter(|rest| !rest.is_empty()) {
        let mut sum = clone::Cloned::default();
        for copy in rest {
            sum.files += copy.cloned.files;
            sum.copied += copy.cloned.copied;
            sum.links_to_main += copy.cloned.links_to_main;
        }
        println!(
            "  copy: {}, {}{}",
            count(rest.len() as u64, "more path"),
            describe_dir(&sum),
            links_to_main(&sum)
        );
    }
    if marked > 0 {
        println!(
            "  copy: marked the tree's {} older than the copies as changed",
            count(marked, "file")
        );
    }
    for (rel, why) in left_out {
        println!("  copy {}: left out, since {why}", rel.display());
    }
}

/// What one copy holds.
fn describe(copy: &Copy) -> String {
    let shown = match copy.kind {
        Kind::File => human_bytes(copy.cloned.copied),
        Kind::Dir => describe_dir(&copy.cloned),
        Kind::Symlink => "a symlink".to_owned(),
    };
    shown + &links_to_main(&copy.cloned)
}

fn describe_dir(cloned: &clone::Cloned) -> String {
    format!(
        "{}, {}",
        count(cloned.files, "file"),
        human_bytes(cloned.copied)
    )
}

/// A warning about the symlinks copied that point into the main checkout, if
/// there are any.
fn links_to_main(cloned: &clone::Cloned) -> String {
    if cloned.links_to_main == 0 {
        return String::new();
    }
    format!(
        "; {} into the main checkout, so a build that writes through one changes it",
        count(cloned.links_to_main, "symlink")
    )
}
