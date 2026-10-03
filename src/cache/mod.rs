//! Build caches grafted into trees, so a new tree doesn't build from cold.
//!
//! A repo declares its cache directories (see [`declare`]), and some build
//! systems' are known without a declaration (see [`ecosystem`]). Each cache
//! is grafted in one of three modes:
//!
//! - `clone`: the main checkout's directory is cloned into the tree, large files
//!   hardlinked and small ones copied (see [`clone`]).
//! - `share`: one directory under the cache dir, symlinked into every tree of
//!   the repo. Only for caches that are content-addressed and safe for several
//!   builds to write at once.
//! - `never`: left cold.
//!
//! workforest only ever replaces or deletes a cache path that git ignores and
//! tracks nothing under, so a mistaken declaration can't destroy source.

mod clone;
pub mod declare;
pub mod doctor;
mod ecosystem;
mod glob;

use std::fmt;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

pub use clone::usage;
pub use declare::declared;

use crate::config::Config;
use crate::error::{Context, Result, bail};
use crate::forest::dir_name;
use crate::git;

/// How a cache path is grafted into a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Clone,
    Share,
    Never,
}

impl Mode {
    fn parse(mode: &str) -> Option<Mode> {
        match mode {
            "clone" => Some(Mode::Clone),
            "share" => Some(Mode::Share),
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
            Mode::Share => "share",
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
/// into `tree`, printing a line for each. `force` replaces caches the tree
/// already has. A cache that can't be grafted is left cold with a warning: the
/// tree works without it.
pub fn graft_tree(config: &Config, tree: &Path, source: &Path, force: bool) {
    let declared = declared(source);
    warn(&declared.warnings);
    for entry in &declared.entries {
        match graft(config, tree, source, entry, force) {
            Ok(Some(note)) => println!("  cache {}: {note}", entry.path),
            Ok(None) => {}
            Err(err) => eprintln!("workforest: cache {}: {err}", entry.path),
        }
    }
}

/// Graft one cache into `tree`, returning what happened, if anything.
pub fn graft(
    config: &Config,
    tree: &Path,
    source: &Path,
    entry: &Entry,
    force: bool,
) -> Result<Option<String>> {
    match entry.mode {
        Mode::Clone => graft_clone(config, tree, source, entry, force).map(Some),
        Mode::Share => graft_share(config, tree, source, entry).map(Some),
        Mode::Never => Ok(None),
    }
}

fn graft_clone(
    config: &Config,
    tree: &Path,
    source: &Path,
    entry: &Entry,
    force: bool,
) -> Result<String> {
    let (src, dst) = (source.join(&entry.path), tree.join(&entry.path));
    check_replaceable(tree, &entry.path, true)?;
    if dst.symlink_metadata().is_ok() {
        if !force {
            return Ok("already there, left alone".to_owned());
        }
        remove(tree, &entry.path)?;
    }
    if !src.is_dir() {
        return Ok("the main checkout has none; starting cold".to_owned());
    }
    let parent = dst.parent().unwrap_or(tree);
    fs::create_dir_all(parent).context(format!("could not create {}", parent.display()))?;
    if !clone::same_filesystem(&src, parent) {
        return Ok(format!(
            "{} is on another filesystem, so it can't be hardlinked; starting cold",
            src.display()
        ));
    }
    match clone::clone_dir(&src, &dst, config.link_min, &entry.always_copy) {
        Ok(cloned) => Ok(format!(
            "grafted from the main checkout: {} files, {} hardlinked, {} copied",
            cloned.files,
            human_bytes(cloned.linked),
            human_bytes(cloned.copied)
        )),
        Err(err) => {
            let _ = fs::remove_dir_all(&dst);
            bail!("could not clone {}: {err}; starting cold", src.display())
        }
    }
}

fn graft_share(config: &Config, tree: &Path, source: &Path, entry: &Entry) -> Result<String> {
    let shared = shared_dir(config, source, &entry.path);
    let dst = tree.join(&entry.path);
    check_replaceable(tree, &entry.path, false)?;
    match dst.symlink_metadata() {
        Ok(meta) if meta.is_symlink() => {
            if fs::read_link(&dst).is_ok_and(|target| target == shared) {
                return Ok(format!("shared, {}", shared.display()));
            }
            fs::remove_file(&dst).context(format!("could not remove {}", dst.display()))?;
        }
        Ok(_) => return Ok("a real directory is in the way, left alone".to_owned()),
        Err(_) => {}
    }
    fs::create_dir_all(&shared).context(format!("could not create {}", shared.display()))?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).context(format!("could not create {}", parent.display()))?;
    }
    symlink(&shared, &dst).context(format!("could not link {}", dst.display()))?;
    Ok(format!("shared, {}", shared.display()))
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
pub fn check_replaceable(tree: &Path, path: &str, dir: bool) -> Result<()> {
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

/// The directory that `share` mode links `path` of the repo at `source` to.
/// It is named after the repo, plus a hash of where the repo lives, so two
/// repos with the same name don't share one.
pub fn shared_dir(config: &Config, source: &Path, path: &str) -> PathBuf {
    let key = format!(
        "{}-{:08x}",
        dir_name(source),
        fnv1a(source.as_os_str().as_encoded_bytes())
    );
    config
        .cache_dir
        .join("share")
        .join(key)
        .join(path.replace('/', "_"))
}

/// A 32-bit FNV-1a hash, which unlike std's hasher is stable across Rust
/// versions, so a shared cache keeps its name.
fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |hash, &byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// `bytes` in the largest binary unit that keeps it at 1 or more.
pub fn human_bytes(bytes: u64) -> String {
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
