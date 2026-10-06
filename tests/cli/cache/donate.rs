//! Caches donated back to the main checkout: moved by `burn` and `cut`,
//! cloned by `cache donate`, and only when the tree's is newer.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::super::Sandbox;
use super::{LINK_MIN, at, meta, other_filesystem, same_file, set_modified, stderr, stdout, write};

/// The inode of `file`.
fn ino(file: &Path) -> u64 {
    meta(file).ino()
}

fn modified(file: &Path) -> SystemTime {
    meta(file).modified().unwrap()
}

/// Seconds since the epoch, `ahead` seconds from now.
fn from_now(ahead: u64) -> u64 {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    now.unwrap().as_secs() + ahead
}

impl Sandbox {
    /// A repo declaring `build clone`, whose main checkout's cache was built
    /// long ago: a large file and a small one, from 2001.
    fn stale_repo(&self, name: &str) -> PathBuf {
        let repo = self.cached_repo(name, "build clone\n");
        for (file, size) in [("big", LINK_MIN), ("small", 10)] {
            write(&repo.join("build").join(file), size);
            set_modified(&repo.join("build").join(file), 1_000_000_000);
        }
        repo
    }
}

#[test]
fn burning_with_donate_cache_moves_a_newer_cache_into_the_main_checkout() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "warm", "repos/api"]);
    let tree = sb.forest("warm").join("api");
    // A build in the tree replaces a large file and adds a small one.
    fs::remove_file(tree.join("build/big")).unwrap();
    write(&tree.join("build/big"), LINK_MIN + 1);
    write(&tree.join("build/new"), 10);
    let (big, new) = (ino(&tree.join("build/big")), ino(&tree.join("build/new")));
    sb.commit(&tree, "work.txt");
    sb.git(&tree, &["push", "--quiet", "-u", "origin", "HEAD"]);

    let out = sb.ok(&sb.root, &["burn", "warm", "--donate-cache"]);

    assert!(
        out.contains(
            "api: donating caches\n  cache build: moved to the main checkout: 3 files, 64.0 KiB"
        ),
        "{out}"
    );
    assert!(out.contains(", replacing one "), "{out}");
    assert!(out.ends_with("burned forest warm\n"), "{out}");
    assert!(!sb.forest("warm").exists());
    // Moved, not copied: the very files the tree built.
    assert_eq!(ino(&repo.join("build/big")), big);
    assert_eq!(ino(&repo.join("build/new")), new);
    assert_eq!(meta(&repo.join("build/small")).len(), 10);
    // The main checkout's sources are older than the output built from the
    // tree's, so they are marked as changed.
    assert!(out.contains("; marked the main checkout's 3 files older than it as changed"));
    assert!(modified(&repo.join("README")) > modified(&repo.join("build/new")));
    assert_eq!(sb.git(&repo, &["status", "--porcelain"]), "");
}

#[test]
fn a_cache_that_built_nothing_new_is_not_donated() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    let big = ino(&repo.join("build/big"));
    let readme = modified(&repo.join("README"));
    sb.ok(&sb.root, &["new", "idle", "repos/api"]);

    let out = sb.ok(&sb.root, &["burn", "idle", "--donate-cache"]);

    assert!(
        out.contains(
            "  cache build: nothing in it is newer than the main checkout's; left alone\n"
        ),
        "{out}"
    );
    assert!(!sb.forest("idle").exists());
    assert_eq!(ino(&repo.join("build/big")), big);
    assert_eq!(modified(&repo.join("README")), readme);
}

#[test]
fn cache_donate_clones_the_cache_and_the_tree_keeps_its_own() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.publish(&repo, ".workforest-cache", "build clone *.lock\n");
    sb.ok(&sb.root, &["new", "live", "repos/api"]);
    let tree = sb.forest("live").join("api");
    fs::remove_file(tree.join("build/big")).unwrap();
    write(&tree.join("build/big"), LINK_MIN + 1);
    write(&tree.join("build/state.lock"), LINK_MIN);
    write(&tree.join("build/new"), 10);
    let before: Vec<_> = ["big", "small", "new", "state.lock"]
        .iter()
        .map(|file| {
            (
                ino(&tree.join("build").join(file)),
                modified(&tree.join("build").join(file)),
            )
        })
        .collect();
    let readme = modified(&tree.join("README"));

    let out = sb.ok(&tree, &["cache", "donate"]);

    assert!(
        out.starts_with(
            "api\n  cache build: cloned to the main checkout: 4 files, 64.0 KiB hardlinked, 64.0 KiB copied"
        ),
        "{out}"
    );
    // The tree's cache is just as it was, sharing its large file.
    let after: Vec<_> = ["big", "small", "new", "state.lock"]
        .iter()
        .map(|file| {
            (
                ino(&tree.join("build").join(file)),
                modified(&tree.join("build").join(file)),
            )
        })
        .collect();
    assert_eq!(before, after);
    assert_eq!(modified(&tree.join("README")), readme);
    assert!(same_file(&tree.join("build/big"), &repo.join("build/big")));
    assert!(!same_file(&tree.join("build/new"), &repo.join("build/new")));
    assert!(!same_file(
        &tree.join("build/state.lock"),
        &repo.join("build/state.lock")
    ));
    assert_eq!(
        modified(&repo.join("build/new")),
        modified(&tree.join("build/new"))
    );
    assert_eq!(sb.git(&repo, &["status", "--porcelain"]), "");

    // Done once, there is nothing newer to give.
    let out = sb.ok(&tree, &["cache", "donate", "api"]);
    assert!(
        out.contains("nothing in it is newer than the main checkout's"),
        "{out}"
    );
}

#[test]
fn a_main_checkouts_newer_output_is_kept_unless_forced() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "rival", "repos/api"]);
    let tree = sb.forest("rival").join("api");
    // The tree builds one thing, and then the main checkout another, as when
    // a second forest's cache was donated meanwhile.
    write(&tree.join("build/ours/out"), 10);
    write(&repo.join("build/theirs/out"), 10);

    let out = sb.ok(&tree, &["cache", "donate"]);

    assert!(
        out.contains(
            "  cache build: the main checkout's theirs is newer than the tree's, so donating would \
             lose it; --force donates anyway\n"
        ),
        "{out}"
    );
    assert!(!repo.join("build/ours").exists());

    let out = sb.ok(&tree, &["cache", "donate", "--force"]);
    assert!(
        out.contains("  cache build: cloned to the main checkout"),
        "{out}"
    );
    assert!(repo.join("build/ours/out").is_file());
    assert!(!repo.join("build/theirs").exists());

    // Built later in the main checkout, the same directory makes the tree's
    // nothing newer.
    write(&repo.join("build/ours/later"), 10);
    let out = sb.ok(&tree, &["cache", "donate"]);
    assert!(
        out.contains("nothing in it is newer than the main checkout's"),
        "{out}"
    );
}

#[test]
fn cargos_profiles_and_targets_are_donated_one_by_one() {
    let sb = Sandbox::new();
    let repo = sb.repo("game");
    sb.publish(&repo, ".gitignore", "/target\n");
    sb.publish(&repo, "Cargo.toml", "[package]\nname = \"game\"\n");
    let android = "target/aarch64-linux-android/debug";
    for file in [
        "target/debug/deps/libold.rlib",
        &format!("{android}/libold.so"),
    ] {
        write(&repo.join(file), LINK_MIN);
        set_modified(&repo.join(file), 1_000_000_000);
    }
    sb.ok(&sb.root, &["new", "desktop", "repos/game"]);
    let tree = sb.forest("desktop").join("game");
    // The tree builds for the desktop, then another forest's Android build
    // is donated to the main checkout.
    write(&tree.join("target/debug/deps/libnew.rlib"), 10);
    write(&repo.join(format!("{android}/libnew.so")), 10);
    sb.commit(&tree, "work.txt");
    sb.git(&tree, &["push", "--quiet", "-u", "origin", "HEAD"]);

    let out = sb.ok(&tree, &["cut", "game", "--donate-cache"]);

    assert!(
        out.contains(
            "  cache target: moved debug to the main checkout: 2 files, 64.0 KiB, newest "
        ),
        "{out}"
    );
    assert!(
        out.contains("; kept the main checkout's newer aarch64-linux-android"),
        "{out}"
    );
    assert!(repo.join("target/debug/deps/libnew.rlib").is_file());
    assert!(repo.join(format!("{android}/libnew.so")).is_file());
    assert!(repo.join(format!("{android}/libold.so")).is_file());
    assert_eq!(sb.git(&repo, &["status", "--porcelain"]), "");
}

#[test]
fn a_main_checkout_built_since_its_head_passed_the_tree_keeps_its_cache() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "behind", "repos/api"]);
    let tree = sb.forest("behind").join("api");
    write(&tree.join("build/out/first"), 10);
    // The main checkout moves on to a commit the tree lacks.
    sb.commit(&repo, "later.txt");

    // Its cache is older than that move, so it isn't for the newer code, and
    // the tree's is donated.
    let out = sb.ok(&tree, &["cache", "donate"]);
    assert!(
        out.contains("  cache build: cloned to the main checkout"),
        "{out}"
    );

    // Then the main checkout builds, and the tree builds again after it.
    write(&repo.join("build/out/main"), 10);
    write(&tree.join("build/out/second"), 10);
    set_modified(&tree.join("build/out/second"), from_now(60));
    let out = sb.ok(&tree, &["cache", "donate"]);
    assert!(
        out.contains(
            "  cache build: the main checkout's HEAD has 1 commit the tree lacks, and it has built \
             since moving there, so its cache may be for newer code; --force donates anyway\n"
        ),
        "{out}"
    );
    assert!(!repo.join("build/out/second").exists());

    // Once the tree has the main checkout's commits, its cache is for code at
    // least as new.
    sb.git(&tree, &["merge", "--quiet", "--no-edit", "main"]);
    let out = sb.ok(&tree, &["cache", "donate"]);
    assert!(
        out.contains("  cache build: cloned to the main checkout"),
        "{out}"
    );
    assert!(repo.join("build/out/second").is_file());
}

#[test]
fn a_cache_a_build_holds_is_left_alone() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "busy", "repos/api"]);
    let tree = sb.forest("busy").join("api");
    write(&tree.join("build/debug/new"), 10);
    write(&tree.join("build/debug/.cargo-lock"), 0);
    write(&repo.join("build/debug/.cargo-lock"), 0);
    set_modified(&repo.join("build/debug/.cargo-lock"), 1_000_000_000);

    for (checkout, held) in [
        (&tree, "in the tree, so its cache may be half written"),
        (&repo, "in the main checkout"),
    ] {
        let lock = fs::File::open(checkout.join("build/debug/.cargo-lock")).unwrap();
        lock.lock().unwrap();

        let out = sb.ok(&tree, &["cache", "donate", "--force"]);

        assert!(
            out.contains(&format!(
                "  cache build: a build holds debug/.cargo-lock {held}; left alone\n"
            )),
            "{out}"
        );
        assert!(!repo.join("build/debug/new").exists());
        drop(lock);
    }

    let out = sb.ok(&tree, &["cache", "donate"]);
    assert!(
        out.contains("  cache build: cloned to the main checkout"),
        "{out}"
    );
}

#[test]
fn only_a_path_git_ignores_in_the_main_checkout_too_is_replaced() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "out clone\n");
    sb.ok(&sb.root, &["new", "ignored", "repos/api"]);
    let tree = sb.forest("ignored").join("api");
    // The tree's branch ignores out, and builds there; the main checkout
    // keeps files of its own there.
    fs::write(tree.join(".gitignore"), "/build\n/out\n").unwrap();
    write(&tree.join("out/built"), 10);
    fs::create_dir(repo.join("out")).unwrap();
    fs::write(repo.join("out/notes"), "mine").unwrap();

    let out = sb.workforest(&tree, &["cache", "donate", "--force"]);

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "cache out: in the main checkout, git doesn't ignore out, so it isn't a cache"
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(fs::read_to_string(repo.join("out/notes")).unwrap(), "mine");
    assert!(!repo.join("out/built").exists());
    assert!(tree.join("out/built").is_file());
}

#[test]
fn what_an_interrupted_donation_left_is_cleared_away() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "again", "repos/api"]);
    let tree = sb.forest("again").join("api");
    write(&tree.join("build/new"), 10);
    write(&repo.join(".build.workforest-donation/half"), 10);
    write(&repo.join(".build.workforest-old/half"), 10);

    let out = sb.ok(&tree, &["cache", "donate"]);

    assert!(
        out.contains("  cache build: cloned to the main checkout"),
        "{out}"
    );
    assert!(!repo.join(".build.workforest-donation").exists());
    assert!(!repo.join(".build.workforest-old").exists());
    assert_eq!(sb.git(&repo, &["status", "--porcelain"]), "");
}

#[test]
fn a_tree_on_another_filesystem_keeps_its_cache() {
    let sb = Sandbox::new();
    let Some(elsewhere) = other_filesystem(&sb.root) else {
        eprintln!("skipped: /dev/shm is missing or on the same filesystem");
        return;
    };
    let _repo = sb.stale_repo("api");
    let root = [("WORKFOREST_ROOT", elsewhere.path().as_os_str())];
    let out = sb.workforest_with(&sb.root, &["new", "far", "repos/api"], &root);
    assert!(out.status.success(), "{}", stderr(&out));
    let tree = elsewhere.path().join("far/api");
    write(&tree.join("build/new"), 10);

    let out = sb.workforest_with(&sb.root, &["cache", "donate", "-f", "far"], &root);

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains(
            "  cache build: the main checkout is on another filesystem, so it can't take the \
             tree's; left alone\n"
        ),
        "{}",
        stdout(&out)
    );
    assert!(tree.join("build/new").is_file());
}

#[test]
fn caches_left_cold_or_never_grafted_have_nothing_to_donate() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\nlogs never\n");
    sb.publish(&repo, ".gitignore", "/build\n/logs\n");
    sb.ok(&sb.root, &["new", "cold", "repos/api", "--no-cache"]);
    let tree = sb.forest("cold").join("api");
    write(&tree.join("logs/today"), 10);

    let out = sb.ok(&tree, &["cache", "donate"]);

    assert_eq!(out, "api\n  cache build: the tree has none\n");
    assert!(!repo.join("logs").exists());
}

#[test]
fn cache_status_shows_how_old_each_cache_is_beside_the_main_checkouts() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "ages", "repos/api"]);
    let tree = sb.forest("ages").join("api");
    set_modified(&tree.join("build/small"), from_now(0) - 3 * 3600);

    let status = sb.ok(&sb.root, &["cache", "status", "ages"]);

    assert!(
        status.contains(", newest 3 h old (the main checkout's: "),
        "{status}"
    );
    let days = (from_now(0) - 1_000_000_000) / 86400;
    assert!(status.contains(&format!("{days} days old)\n")), "{status}");
    assert_eq!(modified(&repo.join("build/big")), at(1_000_000_000));
}
