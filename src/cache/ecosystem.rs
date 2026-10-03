//! Build systems whose caches workforest grafts without being told.
//!
//! An ecosystem is recognized by a file at the repo root, and contributes one
//! cache entry that the repo's declarations can override like any other. Each
//! one is a best guess backed by `workforest cache doctor` runs on real repos:
//! a missing entry costs build time, a wrong one costs a correct build.

use std::path::Path;

use super::{Entry, Mode};

/// A build system, and how to graft its cache.
pub struct Ecosystem {
    pub name: &'static str,
    /// A file at the repo root that says the repo uses this build system.
    pub marker: &'static str,
    /// The cache directory, relative to the repo root.
    pub path: &'static str,
    pub mode: Mode,
    /// Globs of files to copy whatever their size: the ones the build system
    /// rewrites in place, or locks.
    pub always_copy: &'static [&'static str],
    /// The shell command `cache doctor` builds with unless given `--cmd`.
    pub build: &'static str,
}

/// Every ecosystem workforest knows. None yet: each needs evidence that its
/// cache survives grafting.
pub const ECOSYSTEMS: &[Ecosystem] = &[];

impl Ecosystem {
    /// The cache entry this build system contributes, with no origin yet.
    pub fn entry(&self) -> Entry {
        Entry {
            path: self.path.to_owned(),
            mode: self.mode,
            always_copy: self
                .always_copy
                .iter()
                .map(|&glob| glob.to_owned())
                .collect(),
            origin: String::new(),
        }
    }
}

/// The ecosystems the repo whose main worktree is at `source` uses.
pub fn detected(source: &Path) -> impl Iterator<Item = &'static Ecosystem> {
    let source = source.to_path_buf();
    ECOSYSTEMS
        .iter()
        .filter(move |ecosystem| source.join(ecosystem.marker).is_file())
}
