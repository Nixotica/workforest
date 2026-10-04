//! How long a cache graft takes: a reproducible benchmark for #4.
//!
//! Builds a repo whose `target` looks like a cargo build's, scaled down from
//! the ~100 GiB one in #4: many small files, which a graft copies, and fewer
//! large ones, which it hardlinks. Then it plants trees of that repo with and
//! without the graft, under the same directory so on the same filesystem, and
//! reports what the graft cost.
//!
//!     cargo build --release
//!     cargo run --release --example graft_bench -- [options]
//!
//! Options, with their defaults:
//!
//!     --dir <path>         where to build it all: a temporary directory
//!                          under $TMPDIR, which picks the filesystem
//!     --workforest <path>  the binary to time: target/release/workforest
//!     --small <n>          small files, copied: 30000
//!     --large <n>          large files, hardlinked: 8400
//!     --runs <n>           grafts timed: 5
//!
//! The defaults are a fifth of #4's 150k small and 42k large files. Large
//! files are 70 KB each, since a hardlink costs the same whatever the size.
//! Small files average 9 KB, as in #4. Runs after the first read from the
//! page cache; dropping it needs root, which this doesn't ask for.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Options {
    dir: Option<PathBuf>,
    workforest: PathBuf,
    small: u64,
    large: u64,
    runs: u32,
}

fn options() -> Options {
    let mut options = Options {
        dir: None,
        workforest: Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/workforest"),
        small: 30_000,
        large: 8_400,
        runs: 5,
    };
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--dir" => options.dir = Some(PathBuf::from(value)),
            "--workforest" => options.workforest = PathBuf::from(value),
            "--small" => options.small = value.parse().expect("--small takes a number"),
            "--large" => options.large = value.parse().expect("--large takes a number"),
            "--runs" => options.runs = value.parse().expect("--runs takes a number"),
            _ => panic!("unknown option {flag}"),
        }
    }
    options
}

/// A deterministic stream of numbers, so every run builds the same files.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.0 >> 33
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "bench")
        .env("GIT_AUTHOR_EMAIL", "bench@example.com")
        .env("GIT_COMMITTER_NAME", "bench")
        .env("GIT_COMMITTER_EMAIL", "bench@example.com")
        .stdout(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A repo at `dir` with a Cargo.toml, so that workforest grafts its `target`
/// without a declaration, and a `target` of `small` and `large` files.
fn build_repo(dir: &Path, small: u64, large: u64) -> (u64, u64) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet", "--initial-branch=main"]);
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"bench\"\n").unwrap();
    fs::write(dir.join(".gitignore"), "/target\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "--quiet", "--message", "bench"]);

    let debug = dir.join("target/debug");
    let mut random = Lcg(4);
    let (mut small_bytes, mut large_bytes) = (0, 0);
    let pattern: Vec<u8> = (0..=255).cycle().take(80_000).collect();
    // Small files spread over crate-like directories, as fingerprints,
    // dep-info and incremental state are: about twenty to a directory.
    for n in 0..small {
        let size = 1 + random.next() % 18_000;
        let dir = debug.join(format!("incremental/crate-{}", n / 20));
        if n % 20 == 0 {
            fs::create_dir_all(&dir).unwrap();
        }
        fs::write(dir.join(format!("s{n}.bin")), &pattern[..size as usize]).unwrap();
        small_bytes += size;
    }
    let deps = debug.join("deps");
    fs::create_dir_all(&deps).unwrap();
    for n in 0..large {
        fs::write(deps.join(format!("libcrate{n}.rlib")), &pattern[..70_000]).unwrap();
        large_bytes += 70_000;
    }
    (small_bytes, large_bytes)
}

/// Time `workforest new <forest> <repo>`, with `extra` arguments.
fn plant(options: &Options, root: &Path, forest: &str, repo: &Path, extra: &[&str]) -> Duration {
    let start = Instant::now();
    let output = Command::new(&options.workforest)
        .args(["new", forest])
        .arg(repo)
        .args(extra)
        .env("WORKFOREST_ROOT", root)
        .output()
        .expect("run workforest");
    let took = start.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    took
}

fn burn(options: &Options, root: &Path, forest: &str) {
    let status = Command::new(&options.workforest)
        .args(["burn", forest, "--delete-branches"])
        .env("WORKFOREST_ROOT", root)
        .stdout(Stdio::null())
        .status()
        .expect("run workforest");
    assert!(status.success(), "burning {forest} failed");
}

fn main() {
    let options = options();
    assert!(
        options.workforest.is_file(),
        "no workforest at {}: run `cargo build --release`, or pass --workforest",
        options.workforest.display()
    );
    let base = options.dir.clone().unwrap_or_else(env::temp_dir);
    let work = base.join(format!("graft-bench-{}", std::process::id()));
    let repo = work.join("repo");
    let root = work.join("forests");

    let built = Instant::now();
    let (small_bytes, large_bytes) = build_repo(&repo, options.small, options.large);
    eprintln!(
        "built {} small files ({:.0} MiB) and {} large ({:.0} MiB) in {:.1?}",
        options.small,
        mib(small_bytes),
        options.large,
        mib(large_bytes),
        built.elapsed()
    );

    let mut grafts = Vec::new();
    for run in 0..options.runs {
        let cold = plant(
            &options,
            &root,
            &format!("cold{run}"),
            &repo,
            &["--no-cache"],
        );
        let warm = plant(&options, &root, &format!("warm{run}"), &repo, &[]);
        grafts.push(warm.saturating_sub(cold));
        burn(&options, &root, &format!("cold{run}"));
        burn(&options, &root, &format!("warm{run}"));
    }
    fs::remove_dir_all(&work).unwrap();

    grafts.sort();
    let median = grafts[grafts.len() / 2];
    let files = options.small + options.large;
    println!(
        "{files} files: {:.0} MiB copied, {:.0} MiB hardlinked; graft median {:.2} s \
         (fastest {:.2} s) over {} runs, {:.0} files/s",
        mib(small_bytes),
        mib(large_bytes),
        median.as_secs_f64(),
        grafts[0].as_secs_f64(),
        grafts.len(),
        files as f64 / median.as_secs_f64()
    );
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
