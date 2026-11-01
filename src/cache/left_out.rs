//! Directories that a copy of the main checkout's ignored files leaves out:
//! what build systems and tools make inside a checkout that doesn't work, or
//! is worth nothing, at another path. Each ecosystem says how to recognise its
//! own, as [`super::cargo`] says how to graft `target`, by what a directory
//! holds or where it sits rather than by its path, since a project can be
//! anywhere in a repo.

use std::path::Path;

use super::{cmake, gradle, python};

/// A kind of directory that a copy leaves out.
pub struct LeftOut {
    /// Whether the directory at this path in the main checkout is one.
    pub recognizes: fn(&Path) -> bool,
    /// Why, completing "left out, since".
    pub why: &'static str,
}

/// Every kind of directory that a copy leaves out.
const LEFT_OUT: &[LeftOut] = &[
    python::VENV,
    cmake::BUILD,
    gradle::PROJECT_CACHE,
    gradle::BUILD,
    GIT_CHECKOUT,
];

/// A linked worktree or a submodule's checkout, whose `.git` is a file naming
/// git state kept elsewhere, which a copy would share.
const GIT_CHECKOUT: LeftOut = LeftOut {
    recognizes: is_git_checkout,
    why: "a git worktree or submodule names the repository it belongs to",
};

fn is_git_checkout(dir: &Path) -> bool {
    dir.join(".git").is_file()
}

/// Why the directory at `dir` is left out, if it is.
pub fn why(dir: &Path) -> Option<&'static str> {
    LEFT_OUT
        .iter()
        .find(|kind| (kind.recognizes)(dir))
        .map(|kind| kind.why)
}
