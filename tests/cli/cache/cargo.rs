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

    /// Build `gen` in `dir`, then run it and return what it printed.
    fn build_and_run(&self, dir: &Path) -> String {
        let out = self
            .cargo_free(&mut Command::new("cargo"))
            .current_dir(dir)
            .args(["build", "--quiet"])
            .output()
            .expect("run cargo");
        assert!(
            out.status.success(),
            "cargo build failed:\n{}",
            stderr(&out)
        );
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
    let debug = repo.join("target/debug");
    write(&debug.join(".cargo-lock"), 0);
    // Each large enough to be hardlinked by size alone; aws-lc-sys's build
    // script prints over 500 KB of `output`.
    for file in [
        "deps/libapi-0123.rlib",
        "deps/api-0123.d",
        ".fingerprint/api-0123/lib-api",
        "build/api-4567/out/generated.rs",
        "build/api-4567/output",
        "incremental/api-89ab/s-cdef.lock",
    ] {
        write(&debug.join(file), LINK_MIN);
    }

    let out = sb.ok(&sb.root, &["new", "rusty", "repos/api"]);

    assert!(
        out.contains("  cache target: grafted from the main checkout: 7 files"),
        "{out}"
    );
    let grafted = sb.forest("rusty").join("api/target/debug");
    assert!(same_file(
        &debug.join("deps/libapi-0123.rlib"),
        &grafted.join("deps/libapi-0123.rlib")
    ));
    for private in [
        ".cargo-lock",
        "deps/api-0123.d",
        ".fingerprint/api-0123/lib-api",
        "build/api-4567/out/generated.rs",
        "build/api-4567/output",
        "incremental/api-89ab/s-cdef.lock",
    ] {
        assert!(
            !same_file(&debug.join(private), &grafted.join(private)),
            "{private} should be the tree's own"
        );
        assert_eq!(meta(&grafted.join(private)).nlink(), 1);
    }
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
    assert!(report.contains("building: cargo build\n"), "{report}");
    assert!(
        report
            .contains("main checkout        unchanged: the build wrote nothing through a hardlink"),
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
    // A fresh build, a build-script rerun, and a rebuild after an edit, with
    // every file the globs don't name hardlinked, small ones included. Any
    // file Cargo rewrites in place that the globs miss changes the main
    // checkout, and the doctor fails.
    let build = "cargo build && echo B > data.txt && cargo build \
                 && echo '// edit' >> src/main.rs && cargo build";

    let out = sb.workforest_for_cargo(
        &["cache", "doctor", "repos/gen", "--cmd", build],
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
