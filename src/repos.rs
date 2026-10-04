//! Repos named on the command line, and where repos live.
//!
//! A repo argument is a path, absolute or relative to the current directory,
//! or, once `workforest setup` has said where repos live, a bare name such as
//! `api`. A name is looked for in each repos directory, and one level further
//! down in directories that aren't repos themselves, so that `nodal-game`
//! finds `~/repos/nodal/nodal-game`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Config, tilde};
use crate::error::{Result, bail};
use crate::git;

/// The repo that `arg` names: a path, or a name in the repos directories.
pub fn resolve(config: &Config, arg: &str) -> Result<PathBuf> {
    if arg.contains('/') || arg.starts_with('.') {
        return Ok(PathBuf::from(arg));
    }
    let dirs = config.repos()?.value;
    if dirs.is_empty() {
        bail!(
            "{arg} is a name, not a path: pass the repo's path, such as ./{arg}, \
             or run `workforest setup` once to say where your repos live"
        );
    }
    match named(&dirs, arg).as_slice() {
        [repo] => Ok(repo.clone()),
        [] => bail!("no repo named {arg} in {}; pass its path", shown(&dirs)),
        repos => bail!(
            "{arg} names {} repos, {}; pass the path of the one you mean",
            repos.len(),
            shown(repos)
        ),
    }
}

/// The repos called `name` in `dirs`.
fn named(dirs: &[PathBuf], name: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let candidates = [dir.join(name)]
            .into_iter()
            .chain(groups(dir).map(|group| group.join(name)));
        for candidate in candidates {
            if git::is_checkout(&candidate) && !found.contains(&candidate) {
                found.push(candidate);
            }
        }
    }
    found
}

/// The directories in `dir` that aren't hidden and aren't repos themselves,
/// which may hold repos of their own.
fn groups(dir: &Path) -> impl Iterator<Item = PathBuf> {
    subdirs(dir).filter(|sub| !git::is_checkout(sub))
}

fn subdirs(dir: &Path) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
}

/// How many repos `dir` holds where a name would find them.
pub fn count(dir: &Path) -> usize {
    let direct = subdirs(dir).filter(|sub| git::is_checkout(sub)).count();
    let grouped: usize = groups(dir)
        .map(|group| subdirs(&group).filter(|sub| git::is_checkout(sub)).count())
        .sum();
    direct + grouped
}

/// Directories that look like where repos live, most repos first: the home
/// directory if it holds repos itself, and the directories in it that hold at
/// least two.
pub fn suggestions(home: &Path) -> Vec<(PathBuf, usize)> {
    let mut found: Vec<(PathBuf, usize)> = groups(home)
        .map(|dir| {
            let repos = count(&dir);
            (dir, repos)
        })
        .chain([(
            home.to_owned(),
            subdirs(home).filter(|sub| git::is_checkout(sub)).count(),
        )])
        .filter(|(_, repos)| *repos >= 2)
        .collect();
    found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    found.truncate(5);
    found
}

fn shown(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|dir| tilde(dir))
        .collect::<Vec<_>>()
        .join(", ")
}
