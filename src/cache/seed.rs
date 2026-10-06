//! A repo's seed: the build caches its trees graft from and donate back to,
//! kept under the forest root so that workforest never writes to the repo's
//! main checkout.
//!
//! A seed is established the first time a tree of its repo is planted, from the
//! main checkout's caches, and takes the main checkout's again whenever they
//! have newer output: the main checkout is only ever read. Trees donate theirs
//! to it when they are removed, or with `cache donate`, so that trees planted
//! later start from the warmest cache any of them built.
//!
//! The seed of the repo whose main checkout is at `/home/me/repos/api` lives at
//! `<forest root>/.seeds/api-<hash of that path>`, holding each cache at its
//! path in the repo, as `target`, beside workforest's own: a lock, held while
//! anything uses the seed, a record of the main checkout's path and of each
//! cache part's generation, and a directory to stage what it is given in.
//!
//! A part is a whole cache, or, for a cache whose top-level directories are
//! caches of their own (see [`super::Entry::parts`]), one of those. Each time
//! a part is put into the seed it gets a new generation, and a tree records the
//! generations it grafted (see [`Record`]). A donation replaces only the parts
//! still at the generation the tree grafted: one replaced since, by another
//! tree or by the main checkout, holds work the tree's lacks.

use std::collections::BTreeMap;
use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::{Context, Result};
use crate::forest::dir_name;

/// The directory under the forest root that holds every seed.
pub const SEEDS: &str = ".seeds";
/// The seed's record of where it came from and of its parts' generations.
const META: &str = ".workforest-seed";
const LOCK: &str = ".workforest-seed.lock";
/// Where what is given to the seed is staged, and what it replaces is put
/// aside before being deleted.
pub const STAGING: &str = ".workforest-staging";

/// The part key of a cache whose parts aren't caches of their own.
pub const WHOLE: &str = "";

/// Where the seed of the repo whose main checkout is at `source` lives.
pub fn dir(root: &Path, source: &Path) -> PathBuf {
    let hash = fnv1a(source.as_os_str().as_encoded_bytes());
    root.join(SEEDS)
        .join(format!("{}-{:08x}", dir_name(source), hash as u32))
}

/// The 64-bit FNV-1a hash of `bytes`: small, and stable from one build to the
/// next, unlike the standard library's hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A seed in use, locked against any other workforest using it.
pub struct Seed {
    pub dir: PathBuf,
    _lock: File,
    meta: Meta,
}

/// What a seed's record holds.
#[derive(Default)]
pub struct Meta {
    /// The main checkout it seeds.
    pub source: Option<PathBuf>,
    /// Each part's generation, by cache path and part.
    generations: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Seed {
    /// Lock the seed at `dir` for the repo whose main checkout is at
    /// `source`, creating it if need be, and waiting for another workforest
    /// using it to finish.
    pub fn open(dir: &Path, source: &Path) -> Result<Seed> {
        fs::create_dir_all(dir).context(format!("could not create {}", dir.display()))?;
        let lock = lock(dir)?;
        // What an interrupted donation left behind, which may be a whole
        // cache. Nothing else uses the seed while it is locked.
        let staging = dir.join(STAGING);
        if staging.symlink_metadata().is_ok() {
            fs::remove_dir_all(&staging)
                .context(format!("could not remove {}", staging.display()))?;
        }
        let mut meta = Meta::read(dir);
        if meta.source.as_deref() != Some(source) {
            meta.source = Some(source.to_path_buf());
            meta.write(dir)?;
        }
        Ok(Seed {
            dir: dir.to_path_buf(),
            _lock: lock,
            meta,
        })
    }

    /// The generation of `part` of the cache at `cache`: 0 for one the record
    /// doesn't know.
    pub fn generation(&self, cache: &str, part: &str) -> u64 {
        self.meta
            .generations
            .get(cache)
            .and_then(|parts| parts.get(part))
            .copied()
            .unwrap_or(0)
    }

    /// The parts of the cache at `cache` that the seed has, with their
    /// generations: each top-level directory if `by_parts`, else the whole.
    pub fn parts(&self, cache: &str, by_parts: bool) -> BTreeMap<String, u64> {
        part_names(&self.dir.join(cache), by_parts)
            .into_iter()
            .map(|part| {
                let generation = self.generation(cache, &part);
                (part, generation)
            })
            .collect()
    }

    /// Give each of `parts` of the cache at `cache` a new generation, as they
    /// have just been put in place, and forget those of parts no longer there.
    /// Returns the new generation.
    pub fn renew(&mut self, cache: &str, parts: &[String], by_parts: bool) -> Result<u64> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64);
        let known = self.meta.generations.entry(cache.to_owned()).or_default();
        // Strictly increasing, whatever the clock does.
        let generation = (known.values().copied().max().unwrap_or(0) + 1).max(now);
        for part in parts {
            known.insert(part.clone(), generation);
        }
        let present = part_names(&self.dir.join(cache), by_parts);
        known.retain(|part, _| present.contains(part));
        self.meta.write(&self.dir)?;
        Ok(generation)
    }
}

impl Meta {
    /// The record of the seed at `dir`; empty if it has none or it can't be read.
    pub fn read(dir: &Path) -> Meta {
        let mut meta = Meta::default();
        let Ok(text) = fs::read_to_string(dir.join(META)) else {
            return meta;
        };
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields[..] {
                ["source", source] => meta.source = Some(PathBuf::from(source)),
                ["part", cache, part, generation] => {
                    if let Ok(generation) = generation.parse() {
                        meta.generations
                            .entry(cache.to_owned())
                            .or_default()
                            .insert(part.to_owned(), generation);
                    }
                }
                _ => {}
            }
        }
        meta
    }

    /// The cache paths the record knows of.
    pub fn caches(&self) -> impl Iterator<Item = &str> {
        self.generations.keys().map(String::as_str)
    }

    fn write(&self, dir: &Path) -> Result<()> {
        let mut text = String::new();
        if let Some(source) = &self.source {
            text.push_str(&format!("source\t{}\n", source.display()));
        }
        for (cache, parts) in &self.generations {
            for (part, generation) in parts {
                text.push_str(&format!("part\t{cache}\t{part}\t{generation}\n"));
            }
        }
        write_atomically(&dir.join(META), &text)
    }
}

/// The parts of the cache at `dir`: its top-level directories if `by_parts`,
/// else the whole, if it is there.
fn part_names(dir: &Path, by_parts: bool) -> Vec<String> {
    if !by_parts {
        return if dir.is_dir() {
            vec![WHOLE.to_owned()]
        } else {
            Vec::new()
        };
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// What a tree grafted from its repo's seed: for each cache, the generation of
/// each part the seed had then. A cache missing from the record wasn't grafted
/// from a seed, as in a tree planted with `--no-cache`.
///
/// Records live in the forest, at `.workforest-grafts/<tree>`, and go with it.
#[derive(Default)]
pub struct Record {
    path: PathBuf,
    caches: BTreeMap<String, BTreeMap<String, u64>>,
}

/// The directory in a forest that holds its trees' records.
pub const RECORDS: &str = ".workforest-grafts";

impl Record {
    /// The record at `path`; empty if there is none.
    pub fn read(path: &Path) -> Record {
        let mut record = Record {
            path: path.to_path_buf(),
            caches: BTreeMap::new(),
        };
        let Ok(text) = fs::read_to_string(path) else {
            return record;
        };
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields[..] {
                ["cache", cache] => {
                    record.caches.entry(cache.to_owned()).or_default();
                }
                ["part", cache, part, generation] => {
                    if let Ok(generation) = generation.parse() {
                        record
                            .caches
                            .entry(cache.to_owned())
                            .or_default()
                            .insert(part.to_owned(), generation);
                    }
                }
                _ => {}
            }
        }
        record
    }

    /// The parts of the cache at `cache` the tree grafted, with their
    /// generations, if it grafted it from a seed.
    pub fn grafted(&self, cache: &str) -> Option<&BTreeMap<String, u64>> {
        self.caches.get(cache)
    }

    /// Record that the tree grafted `parts` of the cache at `cache`.
    pub fn set(&mut self, cache: &str, parts: BTreeMap<String, u64>) {
        self.caches.insert(cache.to_owned(), parts);
    }

    /// Record that the tree's `part` of the cache at `cache` is the seed's at
    /// `generation`, as after cloning it there.
    pub fn set_part(&mut self, cache: &str, part: &str, generation: u64) {
        if let Some(parts) = self.caches.get_mut(cache) {
            parts.insert(part.to_owned(), generation);
        }
    }

    /// Forget the cache at `cache`, as when the tree's is dropped.
    pub fn forget(&mut self, cache: &str) {
        self.caches.remove(cache);
    }

    pub fn write(&self) -> Result<()> {
        if self.caches.is_empty() {
            return match fs::remove_file(&self.path) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => {
                    Err(err).context(format!("could not remove {}", self.path.display()))
                }
                _ => Ok(()),
            };
        }
        let mut text = String::new();
        for (cache, parts) in &self.caches {
            text.push_str(&format!("cache\t{cache}\n"));
            for (part, generation) in parts {
                text.push_str(&format!("part\t{cache}\t{part}\t{generation}\n"));
            }
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).context(format!("could not create {}", dir.display()))?;
        }
        write_atomically(&self.path, &text)
    }
}

/// Replace the file at `path` with `text`, so that a reader never sees half.
fn write_atomically(path: &Path, text: &str) -> Result<()> {
    let mut staged = path.as_os_str().to_owned();
    staged.push(".tmp");
    let staged = PathBuf::from(staged);
    fs::write(&staged, text).context(format!("could not write {}", staged.display()))?;
    fs::rename(&staged, path).context(format!("could not replace {}", path.display()))
}

/// Every seed under the forest root `root`, as its directory and record,
/// sorted by directory.
pub fn all(root: &Path) -> Result<Vec<(PathBuf, Meta)>> {
    let seeds = root.join(SEEDS);
    let entries = match fs::read_dir(&seeds) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context(format!("could not read {}", seeds.display())),
    };
    let mut found: Vec<(PathBuf, Meta)> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            let dir = entry.path();
            let meta = Meta::read(&dir);
            (dir, meta)
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// Delete the seed at `dir`, once no other workforest is using it.
pub fn remove(dir: &Path) -> Result<()> {
    let _lock = lock(dir)?;
    fs::remove_dir_all(dir).context(format!("could not remove {}", dir.display()))
}

/// Lock the seed at `dir`, waiting for another workforest using it to finish.
fn lock(dir: &Path) -> Result<File> {
    let path = dir.join(LOCK);
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .context(format!("could not open {}", path.display()))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            eprintln!(
                "workforest: waiting for another workforest using {}",
                dir.display()
            );
            lock.lock()
                .context(format!("could not lock {}", path.display()))?;
        }
        Err(TryLockError::Error(err)) => {
            return Err(err).context(format!("could not lock {}", path.display()));
        }
    }
    Ok(lock)
}
