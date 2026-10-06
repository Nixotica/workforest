//! Build systems whose caches workforest grafts without being told.
//!
//! An ecosystem is recognized by a file at the repo root, and contributes one
//! `clone` cache entry that the repo's declarations can override like any
//! other. Each one is a best guess backed by `workforest cache doctor` runs on
//! real repos: a missing entry costs build time, a wrong one costs a correct
//! build.

use std::path::Path;

use super::{Entry, Mode, cargo};

/// A build system, and how to graft its cache.
pub struct Ecosystem {
    pub name: &'static str,
    /// A file at the repo root that says the repo uses this build system.
    pub marker: &'static str,
    /// The cache directory, relative to the repo root.
    pub path: &'static str,
    /// Globs of files to copy whatever their size: the ones the build system
    /// rewrites in place, or locks.
    pub always_copy: &'static [&'static str],
    /// Globs of files that name the checkout their cache was built in without
    /// tying a build elsewhere to it, which `cache doctor` doesn't list for
    /// naming the main checkout when they were grafted rather than written by
    /// the build.
    pub inert: &'static [&'static str],
    /// The shell command `cache doctor` builds with unless given `--cmd`.
    pub build: &'static str,
    /// Whether each top-level directory of the cache is a cache of its own,
    /// which a donation can give up without the others.
    pub parts: bool,
}

/// Every ecosystem workforest knows.
pub const ECOSYSTEMS: &[Ecosystem] = &[cargo::CARGO];

impl Ecosystem {
    /// The cache entry this build system contributes.
    pub fn entry(&self) -> Entry {
        Entry {
            path: self.path.to_owned(),
            mode: Mode::Clone,
            always_copy: self
                .always_copy
                .iter()
                .map(|&glob| glob.to_owned())
                .collect(),
            origin: format!("built in ({})", self.name),
            built_in: true,
            parts: self.parts,
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
