//! Walking a cache directory, the one traversal that cloning, counting and
//! checking a cache all share.

use std::fs::{self, Metadata, ReadDir};
use std::io;
use std::path::{Path, PathBuf};

/// Everything under `root`: each file, directory and symlink as its path
/// relative to `root`, with its metadata. Symlinks aren't followed, and a
/// directory comes before what it holds. An error leaves out the directory it
/// came from, and the walk goes on, so a caller that tolerates unreadable
/// parts can skip errors with `flatten`.
pub fn walk(root: &Path) -> Walk<'static> {
    walk_skipping(root, None)
}

/// As [`walk`], but a directory that `skip` takes, by its path relative to
/// `root`, is left out along with everything in it.
pub fn walk_skipping<'a>(root: &Path, skip: Option<&'a dyn Fn(&Path) -> bool>) -> Walk<'a> {
    Walk {
        root: root.to_path_buf(),
        pending: vec![PathBuf::new()],
        open: None,
        skip,
    }
}

pub struct Walk<'a> {
    root: PathBuf,
    /// Directories found but not read yet.
    pending: Vec<PathBuf>,
    /// The directory being read.
    open: Option<(PathBuf, ReadDir)>,
    skip: Option<&'a dyn Fn(&Path) -> bool>,
}

impl Iterator for Walk<'_> {
    type Item = io::Result<(PathBuf, Metadata)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((dir, entries)) = &mut self.open {
                match entries.next() {
                    Some(Ok(entry)) => {
                        let rel = dir.join(entry.file_name());
                        // Unlike fs::metadata, this doesn't follow symlinks.
                        let meta = match entry.metadata() {
                            Ok(meta) => meta,
                            Err(err) => return Some(Err(err)),
                        };
                        if meta.is_dir() {
                            if self.skip.is_some_and(|skip| skip(&rel)) {
                                continue;
                            }
                            self.pending.push(rel.clone());
                        }
                        return Some(Ok((rel, meta)));
                    }
                    Some(Err(err)) => return Some(Err(err)),
                    None => self.open = None,
                }
            }
            let dir = self.pending.pop()?;
            match fs::read_dir(self.root.join(&dir)) {
                Ok(entries) => self.open = Some((dir, entries)),
                Err(err) => return Some(Err(err)),
            }
        }
    }
}
