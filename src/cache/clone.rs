//! Copying a cache directory into a tree without copying the bulk of its data.
//!
//! Files of `link_min` bytes and up are hardlinked, so they cost no disk.
//! Everything smaller is copied, private to the tree. That split is a bet, not
//! a guarantee: compiled artifacts are large and get replaced wholesale when
//! rebuilt, so sharing them is free, while the files a build system rewrites in
//! place (fingerprints, dep-info, timestamps, locks) are usually small. A large
//! file that is rewritten in place would reach the main checkout through its
//! hardlink, so files matching an always-copy glob are copied whatever their
//! size, and `cache doctor` is how to find the ones that need it.
//!
//! Copies keep their modification times. Build tools judge freshness by
//! comparing mtimes, so a copy stamped with the time of the graft would look
//! newer than the tree's freshly checked-out sources, and stale output built
//! from other sources would pass as fresh.
//!
//! On Linux, `fs::copy` uses `copy_file_range`, which tries a reflink first, so
//! on btrfs or XFS a copy shares its data until written and costs little disk
//! either; on macOS it clones the file on APFS. It is counted as copied all the
//! same.

use std::fs::{self, File, FileTimes, Metadata};
use std::io;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::SystemTime;

use super::glob::Glob;
use super::walk::walk;

/// What a clone cost.
#[derive(Default)]
pub struct Cloned {
    pub files: u64,
    /// Bytes hardlinked, which cost no disk.
    pub linked: u64,
    /// Bytes copied into the tree.
    pub copied: u64,
    /// Symlinks whose absolute target is in the main checkout, so that a
    /// build writing through them changes it.
    pub links_to_main: u64,
    /// The latest modification time of a file cloned, if any was.
    pub newest: Option<SystemTime>,
}

/// Clone the directory `src`, in the main checkout at `main`, to `dst`, which
/// must not exist yet.
///
/// The walk creates each directory before anything in it, and hands files and
/// symlinks to a thread per CPU, at most eight, which copy and link them: a
/// graft is mostly small copies, each a few system calls, and spreading them
/// out is several times faster than one thread on a fast disk.
pub fn clone_dir(
    src: &Path,
    dst: &Path,
    main: &Path,
    link_min: u64,
    always_copy: &[String],
) -> io::Result<Cloned> {
    let always_copy: Vec<Glob> = always_copy.iter().map(|glob| Glob::new(glob)).collect();
    fs::create_dir(dst)?;
    let threads = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8);
    let (send, receive) = mpsc::sync_channel::<(PathBuf, Metadata)>(1024);
    let receive = Mutex::new(receive);
    let failed = AtomicBool::new(false);
    thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut cloned = Cloned::default();
                    loop {
                        let next = receive.lock().ok().and_then(|receive| receive.recv().ok());
                        let Some((rel, meta)) = next else {
                            return Ok(cloned);
                        };
                        let item = Item {
                            rel: &rel,
                            meta: &meta,
                        };
                        if let Err(err) =
                            clone_item(src, dst, main, link_min, &always_copy, item, &mut cloned)
                        {
                            failed.store(true, Ordering::Relaxed);
                            return Err(err);
                        }
                    }
                })
            })
            .collect();
        let walked = (|| {
            for item in walk(src) {
                let (rel, meta) = item?;
                if failed.load(Ordering::Relaxed) {
                    break;
                }
                if meta.is_dir() {
                    fs::create_dir(dst.join(&rel))?;
                } else if send.send((rel, meta)).is_err() {
                    break;
                }
            }
            Ok(())
        })();
        // Closing the channel ends the workers once it is drained.
        drop(send);
        let mut cloned = Cloned::default();
        for worker in workers {
            let done: io::Result<Cloned> = worker
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("a cloning thread panicked")));
            cloned.add(done?);
        }
        walked.map(|()| cloned)
    })
}

/// One file or symlink to clone: its path relative to the cache, and its
/// metadata.
struct Item<'a> {
    rel: &'a Path,
    meta: &'a Metadata,
}

/// Clone one file or symlink, counting it in `cloned`.
fn clone_item(
    src: &Path,
    dst: &Path,
    main: &Path,
    link_min: u64,
    always_copy: &[Glob],
    item: Item,
    cloned: &mut Cloned,
) -> io::Result<()> {
    let Item { rel, meta } = item;
    let (from, to) = (src.join(rel), dst.join(rel));
    let kind = meta.file_type();
    if kind.is_symlink() {
        let target = fs::read_link(&from)?;
        if target.is_absolute() && target.starts_with(main) {
            cloned.links_to_main += 1;
        }
        symlink(target, &to)?;
        cloned.files += 1;
    } else if kind.is_file() {
        cloned.files += 1;
        cloned.newest = cloned.newest.max(meta.modified().ok());
        let link = meta.len() >= link_min && !always_copy.iter().any(|glob| glob.matches(rel));
        // Hardlinking fails once a file has as many links as the filesystem
        // allows; a copy is always safe.
        if link && fs::hard_link(&from, &to).is_ok() {
            cloned.linked += meta.len();
        } else {
            copy_file(&from, &to, meta)?;
            cloned.copied += meta.len();
        }
    }
    // Sockets, FIFOs and devices aren't build output; they stay behind.
    Ok(())
}

impl Cloned {
    /// Count what `other` cloned in this too.
    fn add(&mut self, other: Cloned) {
        self.files += other.files;
        self.linked += other.linked;
        self.copied += other.copied;
        self.links_to_main += other.links_to_main;
        self.newest = self.newest.max(other.newest);
    }
}

/// Copy `from`, whose metadata is `meta`, to `to`, keeping its times.
fn copy_file(from: &Path, to: &Path, meta: &Metadata) -> io::Result<()> {
    fs::copy(from, to)?;
    let times = FileTimes::new()
        .set_accessed(meta.accessed()?)
        .set_modified(meta.modified()?);
    // Setting explicit times needs ownership, not write access, so this works
    // on a read-only copy too.
    File::open(to)?.set_times(times)
}

/// How much of the cache at `dir` is shared with another checkout through
/// hardlinks, and how much is its own.
#[derive(Default)]
pub struct Usage {
    /// Regular files, each counted once.
    pub files: u64,
    pub shared: u64,
    pub own: u64,
}

/// Walk the cache at `dir`. Unreadable parts are left out of the count.
pub fn usage(dir: &Path) -> Usage {
    let mut usage = Usage::default();
    for (_, meta) in walk(dir).flatten() {
        if !meta.is_file() {
            continue;
        }
        usage.files += 1;
        if meta.nlink() > 1 {
            usage.shared += meta.len();
        } else {
            usage.own += meta.len();
        }
    }
    usage
}

/// Whether `a` and `b` are on the same filesystem, so one can hardlink into
/// the other. Anything that can't be read counts as not.
pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        _ => false,
    }
}
