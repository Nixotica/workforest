//! Cargo's `target` directory, which is grafted without a declaration. The
//! tests that build run a real cargo, and are skipped when there is none.

use std::env;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::super::Sandbox;
use super::{LINK_MIN, meta, same_file, stderr, stdout, write};

/// A manifest for a binary package called `gen`, with no dependencies.
const MANIFEST: &str = "\
[package]
name = \"gen\"
version = \"0.1.0\"
edition = \"2021\"
";

const LOCKFILE: &str = "\
version = 4

[[package]]
name = \"gen\"
version = \"0.1.0\"
";

/// A build script that generates a constant from `data.txt`, padded past the
/// size from which grafted files are hardlinked, and writes it in place.
const BUILD_SCRIPT: &str = r#"
fn main() {
    println!("cargo:rerun-if-changed=data.txt");
    let data = std::fs::read_to_string("data.txt").unwrap();
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("generated.rs");
    let padding = "// padding\n".repeat(8000);
    std::fs::write(out, format!("pub const DATA: &str = {:?};\n{padding}", data.trim())).unwrap();
}
"#;

/// A program that prints what the build script generated.
const MAIN: &str = r#"
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
fn main() {
    println!("{DATA}");
}
"#;

impl Sandbox {
    /// Commit `files` to `repo` at once, as `(path, contents)`, and push them.
    fn publish_all(&self, repo: &Path, files: &[(&str, &str)]) {
        for (file, contents) in files {
            let path = repo.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
            self.git(repo, &["add", file]);
        }
        self.git(repo, &["commit", "--quiet", "--message", "publish"]);
        self.git(repo, &["push", "--quiet", "origin", "main"]);
    }

    /// The `gen` package as a repo, generating its output from `data`.
    fn cargo_repo(&self, data: &str) -> PathBuf {
        let repo = self.repo("gen");
        self.publish_all(
            &repo,
            &[
                (".gitignore", "/target\n"),
                ("Cargo.toml", MANIFEST),
                ("Cargo.lock", LOCKFILE),
                ("build.rs", BUILD_SCRIPT),
                ("src/main.rs", MAIN),
                ("data.txt", data),
            ],
        );
        repo
    }

    /// Change `data.txt` on `origin/main`, as someone else's merge would, and
    /// fetch it into `repo` without changing what `repo` has checked out.
    fn move_main_on(&self, repo: &Path, data: &str) {
        let forge = self.forge("gen");
        fs::write(forge.join("data.txt"), data).unwrap();
        self.git(&forge, &["commit", "--quiet", "--all", "--message", data]);
        self.git(&forge, &["push", "--quiet", "origin", "main"]);
        self.git(repo, &["fetch", "--quiet"]);
    }

    /// `command`, isolated, and free of the cargo settings of whatever runs
    /// these tests, such as a target dir that would send builds elsewhere.
    fn cargo_free<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        self.isolate(command);
        for (name, _) in env::vars_os() {
            let name = name.to_string_lossy();
            if (name.starts_with("CARGO_") && name != "CARGO_HOME")
                || name.starts_with("RUSTC_")
                || name.ends_with("RUSTFLAGS")
            {
                command.env_remove(&*name);
            }
        }
        command.env("CARGO_NET_OFFLINE", "true")
    }

    /// Run `cargo` with `args` in `dir`, expecting success.
    fn cargo(&self, dir: &Path, args: &[&str]) {
        let out = self
            .cargo_free(&mut Command::new("cargo"))
            .current_dir(dir)
            .args(args)
            .arg("--quiet")
            .output()
            .expect("run cargo");
        assert!(
            out.status.success(),
            "cargo {args:?} failed:\n{}",
            stderr(&out)
        );
    }

    /// Build `gen` in `dir`, then run it and return what it printed.
    fn build_and_run(&self, dir: &Path) -> String {
        self.cargo(dir, &["build"]);
        let out = Command::new(dir.join("target/debug/gen"))
            .output()
            .expect("run gen");
        stdout(&out).trim().to_owned()
    }

    /// Run workforest, with extra environment variables, without cargo
    /// settings leaking into the builds it runs.
    fn workforest_for_cargo(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workforest"));
        self.cargo_free(&mut command)
            .current_dir(&self.root)
            .args(args)
            .envs(env.iter().copied())
            .output()
            .expect("run workforest")
    }
}

/// Whether there is a cargo to build with; the tests that build are skipped
/// without one.
fn have_cargo() -> bool {
    let found = Command::new("cargo")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    if !found {
        eprintln!("skipped: no cargo on PATH");
    }
    found
}

/// Whether `cargo <subcommand>` runs here: `clippy`, say, which a bare
/// toolchain lacks.
fn cargo_has(subcommand: &str) -> bool {
    Command::new("cargo")
        .args([subcommand, "--version"])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// The file the build script generated in `checkout`.
fn generated(checkout: &Path) -> PathBuf {
    let build = checkout.join("target/debug/build");
    fs::read_dir(&build)
        .unwrap()
        .flatten()
        .map(|entry| entry.path().join("out/generated.rs"))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("nothing generated under {}", build.display()))
}

#[test]
fn cargo_target_is_grafted_without_a_declaration() {
    let sb = Sandbox::new();
    let repo = sb.repo("api");
    sb.publish_all(
        &repo,
        &[(".gitignore", "/target\n"), ("Cargo.toml", MANIFEST)],
    );
    let target = repo.join("target");
    let locks = [
        "debug/.cargo-lock",
        "debug/.cargo-build-lock",
        "debug/.cargo-artifact-lock",
    ];
    for lock in locks {
        write(&target.join(lock), 0);
    }
    // Each large enough to be hardlinked by size alone; aws-lc-sys's build
    // script prints over 500 KB of `output`.
    let private = [
        "debug/deps/api-0123.d",
        "debug/.fingerprint/api-0123/lib-api",
        "debug/build/api-4567/out/generated.rs",
        "debug/build/api-4567/output",
        "debug/incremental/api-89ab/s-cdef.lock",
        ".rustc_info.json",
        "doc/static.files/search-0123.js",
        "doc/trait.impl/core/marker/trait.Send.js",
    ];
    for file in private.iter().chain(&["debug/deps/libapi-0123.rlib"]) {
        write(&target.join(file), LINK_MIN);
    }

    let out = sb.ok(&sb.root, &["new", "rusty", "repos/api"]);

    assert!(
        out.contains("  cache target: grafted from the seed: 12 files"),
        "{out}"
    );
    let grafted = sb.forest("rusty").join("api/target");
    assert!(same_file(
        &target.join("debug/deps/libapi-0123.rlib"),
        &grafted.join("debug/deps/libapi-0123.rlib")
    ));
    for file in private.iter().chain(&locks) {
        assert!(
            !same_file(&target.join(file), &grafted.join(file)),
            "{file} should be the tree's own"
        );
        assert_eq!(meta(&grafted.join(file)).nlink(), 1);
    }
}

#[test]
fn a_cargo_repo_whose_main_checkout_has_no_target_is_passed_quietly() {
    let sb = Sandbox::new();
    let repo = sb.repo("api");
    sb.publish_all(
        &repo,
        &[(".gitignore", "/target\n"), ("Cargo.toml", MANIFEST)],
    );

    let out = sb.workforest(&sb.root, &["new", "quiet", "repos/api"]);

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("cache"), "{}", stdout(&out));
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));

    // A declared cache the main checkout lacks is still worth a line.
    fs::write(repo.join(".git/workforest-cache"), "target clone\n").unwrap();
    let out = sb.ok(&sb.root, &["new", "declared", "repos/api"]);
    assert!(
        out.contains("cache target: the main checkout has none; starting cold"),
        "{out}"
    );
}

#[test]
fn declarations_override_the_built_in_cargo_entry() {
    let sb = Sandbox::new();
    let repo = sb.repo("api");
    sb.publish_all(
        &repo,
        &[(".gitignore", "/target\n"), ("Cargo.toml", MANIFEST)],
    );
    write(&repo.join("target/debug/big"), LINK_MIN);

    let target = |paths: &str| -> Vec<String> {
        paths
            .lines()
            .find(|line| line.trim_start().starts_with("target "))
            .unwrap_or_else(|| panic!("no target in:\n{paths}"))
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    };
    let paths = sb.ok(&sb.root, &["cache", "paths", "repos/api"]);
    let built_in = target(&paths);
    assert_eq!(built_in[..2], ["target", "clone"], "{paths}");
    assert_eq!(built_in[3..], ["built", "in", "(cargo)"], "{paths}");
    let globs = built_in[2].clone();
    for glob in [
        ".cargo-lock",
        "*.d",
        ".fingerprint/*",
        "build/*/out/*",
        "build/*/output",
    ] {
        assert!(
            globs.split(',').any(|g| g == glob),
            "{glob} missing from {globs}"
        );
    }

    // Overriding the mode alone keeps cargo's globs.
    fs::write(repo.join(".git/workforest-cache"), "target clone\n").unwrap();
    let paths = sb.ok(&sb.root, &["cache", "paths", "repos/api"]);
    assert_eq!(
        target(&paths),
        ["target", "clone", &globs, ".git/workforest-cache:1"],
        "{paths}"
    );

    fs::write(repo.join(".git/workforest-cache"), "target never\n").unwrap();
    sb.ok(&sb.root, &["new", "cold", "repos/api"]);
    assert!(!sb.forest("cold").join("api/target").exists());

    let plain = sb.repo("plain");
    write(&plain.join("target/big"), LINK_MIN);
    let paths = sb.ok(&sb.root, &["cache", "paths", "repos/plain"]);
    assert!(paths.contains("no caches declared"), "{paths}");
}

#[test]
fn a_tree_rebuilds_what_its_sources_changed_instead_of_trusting_the_graft() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    let repo = sb.cargo_repo("A");
    assert_eq!(sb.build_and_run(&repo), "A");
    // origin/main moves on to B, while the main checkout and its cache stay on A.
    sb.move_main_on(&repo, "B");

    sb.ok(&sb.root, &["new", "next", "repos/gen"]);
    let tree = sb.forest("next").join("gen");

    assert_eq!(
        sb.build_and_run(&tree),
        "B",
        "the tree ran output built from A"
    );
    assert_eq!(sb.build_and_run(&repo), "A");
}

#[test]
fn a_later_graft_cannot_pass_the_main_checkouts_output_off_as_the_trees() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    let repo = sb.cargo_repo("A");
    sb.ok(&sb.root, &["new", "later", "repos/gen", "--no-cache"]);
    let tree = sb.forest("later").join("gen");
    // After the tree is planted, the main checkout's sources change and it
    // builds, so its output is newer than the tree's sources.
    fs::write(repo.join("data.txt"), "M").unwrap();
    assert_eq!(sb.build_and_run(&repo), "M");

    sb.ok(&tree, &["cache", "graft"]);

    assert_eq!(
        sb.build_and_run(&tree),
        "A",
        "the tree ran output built from the main checkout's sources"
    );
}

#[test]
fn build_scripts_rewriting_their_output_leave_the_main_checkout_alone() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    let repo = sb.cargo_repo("A");
    assert_eq!(sb.build_and_run(&repo), "A");
    let main_generated = generated(&repo);
    let before = meta(&main_generated);
    assert!(
        before.len() >= LINK_MIN as u64,
        "the generated file must be large"
    );
    sb.move_main_on(&repo, "B");

    sb.ok(&sb.root, &["new", "next", "repos/gen"]);
    let tree = sb.forest("next").join("gen");
    assert_eq!(sb.build_and_run(&tree), "B");

    let after = meta(&main_generated);
    assert_eq!(
        (after.ino(), after.mtime(), after.mtime_nsec()),
        (before.ino(), before.mtime(), before.mtime_nsec()),
        "the tree's build script wrote into the main checkout's OUT_DIR"
    );
    assert!(
        fs::read_to_string(&main_generated)
            .unwrap()
            .contains("\"A\"")
    );
    assert_eq!(sb.build_and_run(&repo), "A");
}

#[test]
fn doctor_builds_cargo_repos_with_cargo_by_default() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    let repo = sb.cargo_repo("A");
    sb.build_and_run(&repo);

    let out = sb.workforest_for_cargo(&["cache", "doctor", "repos/gen"], &[]);

    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let report = stdout(&out);
    assert!(
        report.contains("building: cargo build --all-targets && cargo doc\n"),
        "{report}"
    );
    // Not rustc's superseded incremental sessions, which name the main
    // checkout but go unread.
    assert!(
        report.contains("main checkout's path no grafted file names the main checkout\n"),
        "{report}"
    );

    // What the build writes there is listed all the same.
    let build = format!(
        "cargo build && mkdir -p target/debug/incremental/later \
         && printf %s '{}' > target/debug/incremental/later/naming.o",
        repo.display()
    );
    let out = sb.workforest_for_cargo(&["cache", "doctor", "repos/gen", "--cmd", &build], &[]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let report = stdout(&out);
    assert!(
        report.contains("1 file naming the main checkout")
            && report.contains("    target/debug/incremental/later/naming.o\n"),
        "{report}"
    );
    assert!(
        report
            .contains("main checkout        unchanged: the build wrote nothing through a hardlink"),
        "{report}"
    );
}

#[test]
fn doctor_leaves_out_what_cargo_records_of_the_main_checkout_harmlessly() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    // A dependency outside the repo, with a build script, that the tree
    // doesn't rebuild, as it wouldn't a registry crate: its dep-info and its
    // build script's `root-output` keep naming the main checkout.
    let shared = sb.root.join("shared");
    for (file, contents) in [
        (
            "Cargo.toml",
            "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("build.rs", "fn main() {}\n"),
        ("src/lib.rs", "pub const SHARED: u8 = 1;\n"),
    ] {
        fs::create_dir_all(shared.join(file).parent().unwrap()).unwrap();
        fs::write(shared.join(file), contents).unwrap();
    }
    let repo = sb.repo("gen");
    let manifest = format!(
        "{MANIFEST}\n[dependencies]\nshared = {{ path = {:?} }}\n",
        shared.display().to_string()
    );
    sb.publish_all(
        &repo,
        &[
            (".gitignore", "/target\n/Cargo.lock\n"),
            ("Cargo.toml", &manifest),
            ("build.rs", BUILD_SCRIPT),
            ("src/main.rs", MAIN),
            ("data.txt", "A"),
        ],
    );
    assert_eq!(sb.build_and_run(&repo), "A");

    let out = sb.workforest_for_cargo(&["cache", "doctor", "repos/gen"], &[]);

    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let report = stdout(&out);
    assert!(
        report.contains("main checkout's path no grafted file names the main checkout\n"),
        "{report}"
    );
}

#[test]
fn cargos_globs_keep_the_main_checkout_safe_with_everything_else_hardlinked() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    let repo = sb.cargo_repo("A");
    sb.build_and_run(&repo);
    sb.cargo(&repo, &["doc"]);
    // A fresh build, a build-script rerun, a rebuild after an edit, docs, and
    // clippy taking turns with a build, with every file the globs don't name
    // hardlinked, small ones included. Any file Cargo rewrites in place that
    // the globs miss changes the main checkout, and the doctor fails.
    let mut build = "cargo build && echo B > data.txt && cargo build \
                     && echo '// edit' >> src/main.rs && cargo build && cargo doc"
        .to_owned();
    if cargo_has("clippy") {
        build.push_str(" && cargo clippy && cargo build");
    } else {
        eprintln!("not testing clippy: cargo has no clippy here");
    }

    let out = sb.workforest_for_cargo(
        &["cache", "doctor", "repos/gen", "--cmd", &build],
        &[("WORKFOREST_CACHE_LINK_MIN", "0")],
    );

    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out)
            .contains("main checkout        unchanged: the build wrote nothing through a hardlink"),
        "{}",
        stdout(&out)
    );
}

/// A build script that says it has started, then waits for a file called `go`
/// beside the manifest, holding the build, and its lock, until then.
const WAITING_BUILD_SCRIPT: &str = r#"
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    std::fs::write(dir.join("started"), "").unwrap();
    for _ in 0..600 {
        if dir.join("go").exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("never told to go");
}
"#;

/// A `cargo build` in progress, held in its build script until told to go.
struct Held {
    dir: PathBuf,
    cargo: std::process::Child,
}

impl Sandbox {
    /// Start a build in `dir` of a package with [`WAITING_BUILD_SCRIPT`], and
    /// return once it is waiting in the build script.
    fn hold_build(&self, dir: &Path) -> Held {
        let _ = fs::remove_file(dir.join("started"));
        let _ = fs::remove_file(dir.join("go"));
        let mut cargo = self
            .cargo_free(&mut Command::new("cargo"))
            .current_dir(dir)
            .args(["build", "--quiet"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("run cargo");
        for _ in 0..1200 {
            if dir.join("started").exists() {
                return Held {
                    dir: dir.to_path_buf(),
                    cargo,
                };
            }
            if let Some(status) = cargo.try_wait().unwrap() {
                panic!("cargo finished before its build script started: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = cargo.kill();
        panic!("cargo's build script never started in {}", dir.display());
    }
}

impl Held {
    /// Let the build finish, expecting it to succeed.
    fn finish(mut self) {
        fs::write(self.dir.join("go"), "").unwrap();
        let status = self.cargo.wait().unwrap();
        assert!(status.success(), "cargo build in {}", self.dir.display());
    }
}

/// The host's target triple, as rustc reports it.
fn host_triple() -> String {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .expect("run rustc");
    stdout(&out)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("rustc names its host")
        .to_owned()
}

/// Every file under `dir` by path, with what a write to it would change:
/// inode, modification time and size.
fn stamps(dir: &Path) -> std::collections::BTreeMap<PathBuf, (u64, i64, i64, u64)> {
    let mut stamps = std::collections::BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap().flatten() {
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                let stamp = (meta.ino(), meta.mtime(), meta.mtime_nsec(), meta.size());
                stamps.insert(entry.path(), stamp);
            }
        }
    }
    stamps
}

/// Commit what `tree` changed and push it, so that it can be burned.
fn push_work(sb: &Sandbox, tree: &Path) {
    sb.git(tree, &["commit", "--quiet", "--all", "--message", "work"]);
    sb.git(tree, &["push", "--quiet", "-u", "origin", "HEAD"]);
}

#[test]
fn a_target_donated_to_the_seed_leaves_the_next_tree_building_its_own_sources() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    let repo = sb.cargo_repo("A");
    assert_eq!(sb.build_and_run(&repo), "A");
    sb.ok(&sb.root, &["new", "change", "repos/gen"]);
    let tree = sb.forest("change").join("gen");
    fs::write(tree.join("data.txt"), "B").unwrap();
    assert_eq!(sb.build_and_run(&tree), "B");
    push_work(&sb, &tree);
    let main = stamps(&repo.join("target"));

    let out = sb.workforest_for_cargo(&["burn", "change"], &[]);

    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).contains("  cache target: moved debug to the seed: "),
        "{}",
        stdout(&out)
    );
    assert!(
        stamps(&repo.join("target")) == main,
        "the main checkout's cache changed"
    );
    sb.ok(&sb.root, &["new", "next", "repos/gen"]);
    let next = sb.forest("next").join("gen");
    assert!(
        fs::read_to_string(generated(&next))
            .unwrap()
            .contains("\"B\""),
        "the next tree holds what the last one built"
    );
    assert_eq!(
        sb.build_and_run(&next),
        "A",
        "the next tree ran output built from the last one's sources"
    );
    assert_eq!(sb.build_and_run(&repo), "A");
}

#[test]
fn a_live_donation_leaves_each_checkout_building_its_own_sources() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    let repo = sb.cargo_repo("A");
    assert_eq!(sb.build_and_run(&repo), "A");
    sb.ok(&sb.root, &["new", "live", "repos/gen"]);
    let tree = sb.forest("live").join("gen");
    fs::write(tree.join("data.txt"), "B").unwrap();
    assert_eq!(sb.build_and_run(&tree), "B");
    let built = meta(&tree.join("target/debug/gen")).modified().unwrap();
    let before = stamps(&tree.join("target"));
    let main = stamps(&repo.join("target"));

    let out = sb.workforest_for_cargo(&["cache", "donate", "-f", "live"], &[]);

    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).contains("  cache target: cloned debug to the seed: "),
        "{}",
        stdout(&out)
    );
    sb.ok(&sb.root, &["new", "next", "repos/gen"]);
    let next = sb.forest("next").join("gen");
    assert_eq!(sb.build_and_run(&next), "A");
    assert!(
        stamps(&tree.join("target")) == before,
        "the next tree's build wrote through to the donor's cache"
    );
    assert!(
        stamps(&repo.join("target")) == main,
        "the main checkout's cache changed"
    );
    assert_eq!(sb.build_and_run(&tree), "B");
    assert_eq!(
        meta(&tree.join("target/debug/gen")).modified().unwrap(),
        built,
        "the tree had to rebuild after donating"
    );
}

#[test]
fn a_running_cargo_build_keeps_its_cache_out_of_the_seed() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    let repo = sb.repo("gen");
    sb.publish_all(
        &repo,
        &[
            (".gitignore", "/target\n/started\n/go\n"),
            ("Cargo.toml", MANIFEST),
            ("Cargo.lock", LOCKFILE),
            ("build.rs", WAITING_BUILD_SCRIPT),
            ("src/main.rs", "fn main() {}\n"),
        ],
    );
    sb.ok(&sb.root, &["new", "busy", "repos/gen"]);
    let tree = sb.forest("busy").join("gen");
    let donate = ["cache", "donate", "-f", "busy", "--force"];

    let build = sb.hold_build(&tree);
    let out = sb.workforest_for_cargo(&donate, &[]);
    build.finish();

    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        holds_cargo_lock(
            &stdout(&out),
            "in the tree, so its cache may be half written; left alone"
        ),
        "{}",
        stdout(&out)
    );
    assert!(!sb.seed(&repo).join("target").exists());

    let out = sb.workforest_for_cargo(&donate, &[]);
    assert!(
        stdout(&out).contains("  cache target: cloned to the seed: "),
        "{}",
        stdout(&out)
    );

    // A build in the main checkout keeps the seed from taking its newer
    // cache, and a tree planted meanwhile grafts the seed's as it is.
    let build = sb.hold_build(&repo);
    let out = sb.workforest_for_cargo(&["new", "meanwhile", "repos/gen"], &[]);
    build.finish();

    assert!(
        holds_cargo_lock(
            &stdout(&out),
            "in the main checkout, so the seed keeps what it has"
        ),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains("  cache target: grafted from the seed: "),
        "{}",
        stdout(&out)
    );
}

/// Whether `report` says a build holds one of Cargo's build locks, and then
/// `rest`: `.cargo-lock`, or one Cargo 1.98 added.
fn holds_cargo_lock(report: &str, rest: &str) -> bool {
    [".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"]
        .iter()
        .any(|lock| {
            report.contains(&format!(
                "  cache target: a build holds debug/{lock} {rest}\n"
            ))
        })
}

#[test]
fn profiles_and_target_triples_from_different_trees_build_the_next_trees_sources() {
    if !have_cargo() {
        return;
    }
    let sb = Sandbox::new();
    let host = host_triple();
    let run = |checkout: &Path, triple: Option<&str>| -> String {
        let mut args = vec!["build"];
        let mut binary = checkout.join("target");
        if let Some(triple) = triple {
            args.extend(["--target", triple]);
            binary.push(triple);
        }
        sb.cargo(checkout, &args);
        let out = Command::new(binary.join("debug/gen")).output().unwrap();
        stdout(&out).trim().to_owned()
    };
    // The main checkout builds for its host, and for a target triple, as
    // for a phone: build scripts for the host under `debug`, the rest under
    // the triple's directory.
    let repo = sb.cargo_repo("A");
    assert_eq!(run(&repo, None), "A");
    assert_eq!(run(&repo, Some(&host)), "A");
    sb.ok(&sb.root, &["new", "desk", "repos/gen"]);
    sb.ok(&sb.root, &["new", "phone", "repos/gen"]);
    let (desk, phone) = (
        sb.forest("desk").join("gen"),
        sb.forest("phone").join("gen"),
    );
    fs::write(desk.join("data.txt"), "B").unwrap();
    assert_eq!(run(&desk, None), "B");
    push_work(&sb, &desk);
    // The phone's build runs the host's build scripts too, after the desk's.
    fs::write(phone.join("data.txt"), "C").unwrap();
    assert_eq!(run(&phone, Some(&host)), "C");
    push_work(&sb, &phone);
    let main = stamps(&repo.join("target"));

    let out = sb.workforest_for_cargo(&["burn", "desk"], &[]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).contains("  cache target: moved debug to the seed: "),
        "{}",
        stdout(&out)
    );
    let out = sb.workforest_for_cargo(&["burn", "phone"], &[]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let report = stdout(&out);
    assert!(
        report.contains(&format!("  cache target: moved {host} to the seed: ")),
        "{report}"
    );
    assert!(
        report.contains("; kept the seed's debug, changed since this tree grafted it"),
        "{report}"
    );

    sb.ok(&sb.root, &["new", "next", "repos/gen"]);
    let next = sb.forest("next").join("gen");
    assert_eq!(run(&next, None), "A");
    assert_eq!(run(&next, Some(&host)), "A");
    assert!(
        stamps(&repo.join("target")) == main,
        "the main checkout's cache changed"
    );
}
