//! Which caches a repo declares, and where each declaration comes from.
//!
//! A declaration file lists one cache per line: its path relative to the repo
//! root, a mode (default `clone`), and the globs of files to copy whatever their
//! size, comma-separated, or `-` for none. `#` starts a comment anywhere on a
//! line, so a path or glob can't contain one.
//!
//! ```text
//! # path   mode    always-copy globs
//! build    clone   *.lock,state/*
//! .venv    never
//! ```
//!
//! Entries come in layers, each overriding the one before for the same path:
//! `.workforest-cache` at the repo root, committed with the repo, then
//! `workforest-cache` in the git common dir, which is machine-local.

use std::fs;
use std::io;
use std::path::{Component, Path};

use super::{Entry, Mode};
use crate::forest::dir_name;
use crate::git;

/// The declaration file committed at a repo's root.
pub const REPO_FILE: &str = ".workforest-cache";
/// The machine-local declaration file in a repo's git common dir.
pub const LOCAL_FILE: &str = "workforest-cache";

/// The caches a repo declares, and the lines that were ignored.
#[derive(Default)]
pub struct Declared {
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
}

/// What one line declares.
struct Line {
    path: String,
    mode: Mode,
    /// The globs the line lists: `None` if it has no globs field, empty for `-`.
    always_copy: Option<Vec<String>>,
}

/// The caches the repo whose main worktree is at `source` declares.
pub fn declared(source: &Path) -> Declared {
    let mut declared = Declared::default();
    declared.read(source, &source.join(REPO_FILE));
    if let Some(common) = git::common_dir(source) {
        declared.read(source, &common.join(LOCAL_FILE));
    }
    declared
}

impl Declared {
    /// Layer the entries in `file`, if it exists, over those so far.
    fn read(&mut self, source: &Path, file: &Path) {
        // Name the file relative to the repo, as in `.git/workforest-cache`.
        let shown = file.strip_prefix(source).unwrap_or(file).display();
        let repo = dir_name(source);
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return,
            Err(err) => {
                self.warnings
                    .push(format!("{repo}: could not read {shown}: {err}"));
                return;
            }
        };
        for (n, line) in text.lines().enumerate() {
            let origin = format!("{shown}:{}", n + 1);
            let line = line.split('#').next().unwrap_or_default();
            match parse(line) {
                Ok(Some(line)) => self.layer(line, origin),
                Ok(None) => {}
                Err(why) => self
                    .warnings
                    .push(format!("{repo}: {origin}: ignoring {why}")),
            }
        }
    }

    /// Add what `line`, declared at `origin`, declares, replacing any earlier
    /// entry for the same path.
    fn layer(&mut self, line: Line, origin: String) {
        let earlier = self
            .entries
            .iter_mut()
            .find(|entry| entry.path == line.path);
        // A line that doesn't list globs keeps those of the entry it
        // overrides, so overriding a path's mode doesn't drop its globs.
        let always_copy = line
            .always_copy
            .or_else(|| earlier.as_ref().map(|entry| entry.always_copy.clone()))
            .unwrap_or_default();
        let entry = Entry {
            path: line.path,
            mode: line.mode,
            always_copy,
            origin,
        };
        match earlier {
            Some(earlier) => *earlier = entry,
            None => self.entries.push(entry),
        }
    }
}

/// What one line declares, or nothing for a blank line.
fn parse(line: &str) -> Result<Option<Line>, String> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let (path, mode, globs) = match fields[..] {
        [] => return Ok(None),
        [path] => (path, None, None),
        [path, mode] => (path, Some(mode), None),
        [path, mode, globs] => (path, Some(mode), Some(globs)),
        _ => return Err("a line with more than three fields".to_owned()),
    };
    let path = valid_path(path).map_err(|why| format!("cache path {path}: {why}"))?;
    let mode = match mode {
        None => Mode::Clone,
        Some(mode) => {
            Mode::parse(mode).ok_or_else(|| format!("unknown cache mode {mode} for {path}"))?
        }
    };
    let always_copy = globs.map(|globs| match globs {
        "-" => Vec::new(),
        globs => globs
            .split(',')
            .filter(|glob| !glob.is_empty())
            .map(str::to_owned)
            .collect(),
    });
    Ok(Some(Line {
        path,
        mode,
        always_copy,
    }))
}

/// `path`, normalized, if it is safe to use as a cache path: workforest
/// deletes and replaces cache paths, so one must stay inside the tree and
/// clear of git's own files.
pub fn valid_path(path: &str) -> Result<String, &'static str> {
    let mut parts = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) if part == ".git" => return Err("it is git's own"),
            Component::Normal(part) => parts.push(part.to_string_lossy()),
            Component::CurDir => {}
            Component::ParentDir => return Err("it leaves the tree"),
            Component::RootDir | Component::Prefix(_) => return Err("it is absolute"),
        }
    }
    if parts.is_empty() {
        return Err("it is the whole tree");
    }
    Ok(parts.join("/"))
}
