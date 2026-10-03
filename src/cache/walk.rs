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
pub fn walk(root: &Path) -> Walk {
    Walk {
        root: root.to_path_buf(),
        pending: vec![PathBuf::new()],
        open: None,
    }
}

pub struct Walk {
    root: PathBuf,
    /// Directories found but not read yet.
    pending: Vec<PathBuf>,
    /// The directory being read.
    open: Option<(PathBuf, ReadDir)>,
}

impl Iterator for Walk {
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
