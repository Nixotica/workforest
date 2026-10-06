//! Repos' seeds, under the forest root: established from the main checkout's
//! caches, grafted into trees, and given trees' caches back by `burn`, `cut`,
//! `fire` and `cache donate`, only where they are newer and the seed hasn't
//! changed since the tree grafted it. The main checkout is only ever read.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::super::{Merge, Sandbox};
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

/// Everything under `dir` but `.git`, with what a write to it would change:
/// whether it is a directory, its inode, modification time and size.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (bool, u64, i64, i64, u64)> {
    let mut found = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap().flatten() {
            let path = entry.path();
            if path == dir.join(".git") {
                continue;
            }
            let meta = meta(&path);
            if meta.is_dir() {
                pending.push(path.clone());
            }
            let stamp = (
                meta.is_dir(),
                meta.ino(),
                meta.mtime(),
                meta.mtime_nsec(),
                meta.len(),
            );
            found.insert(path.strip_prefix(dir).unwrap().to_path_buf(), stamp);
        }
    }
    found
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
fn the_seed_carries_a_trees_cache_to_the_next_and_the_main_checkout_is_only_read() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    let main = snapshot(&repo);

    let out = sb.ok(&sb.root, &["new", "warm", "repos/api"]);

    assert!(
        out.contains(
            "  cache build: seeded from the main checkout: 2 files, 64.0 KiB hardlinked, 10 B \
             copied\n  cache build: grafted from the seed: 2 files, 64.0 KiB hardlinked, 10 B \
             copied\n"
        ),
        "{out}"
    );
    let tree = sb.forest("warm").join("api");
    // A build in the tree replaces a large file and adds a small one.
    fs::remove_file(tree.join("build/big")).unwrap();
    write(&tree.join("build/big"), LINK_MIN + 1);
    write(&tree.join("build/new"), 10);
    let (big, new) = (ino(&tree.join("build/big")), ino(&tree.join("build/new")));
    sb.commit(&tree, "work.txt");
    sb.git(&tree, &["push", "--quiet", "-u", "origin", "HEAD"]);

    let out = sb.ok(&sb.root, &["burn", "warm"]);

    assert!(
        out.contains(
            "api: donating caches\n  cache build: moved to the seed: 3 files, 64.0 KiB, newest "
        ),
        "{out}"
    );
    assert!(out.contains(", replacing one "), "{out}");
    assert!(out.ends_with("burned forest warm\n"), "{out}");
    // Moved, not copied: the very files the tree built.
    let seed = sb.seed(&repo);
    assert_eq!(ino(&seed.join("build/big")), big);
    assert_eq!(ino(&seed.join("build/new")), new);
    assert_eq!(snapshot(&repo), main, "the main checkout changed");
    assert_eq!(sb.git(&repo, &["status", "--porcelain"]), "");

    // The next tree grafts what the last one built, and the main checkout's
    // older cache stays out of it.
    let out = sb.ok(&sb.root, &["new", "next", "repos/api"]);

    assert!(
        out.contains("\n  cache build: grafted from the seed: 3 files, "),
        "{out}"
    );
    assert!(!out.contains("main checkout"), "{out}");
    let next = sb.forest("next").join("api");
    assert!(same_file(&seed.join("build/big"), &next.join("build/big")));
    assert!(next.join("build/new").is_file());
    assert_eq!(snapshot(&repo), main, "the main checkout changed");
}

#[test]
fn a_cache_with_nothing_newer_or_no_donate_cache_leaves_the_seed_alone() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "idle", "repos/api"]);
    sb.ok(&sb.root, &["new", "kept", "repos/api"]);
    let seed = sb.seed(&repo);
    let big = ino(&seed.join("build/big"));

    // Removing a tree says nothing of a cache with nothing to give;
    // `cache donate` says why.
    let idle = sb.forest("idle").join("api");
    let out = sb.ok(&idle, &["cache", "donate"]);
    assert_eq!(
        out,
        "api\n  cache build: nothing in it is newer than the seed's; left alone\n"
    );
    assert_eq!(sb.ok(&sb.root, &["burn", "idle"]), "burned forest idle\n");

    let kept = sb.forest("kept").join("api");
    write(&kept.join("build/new"), 10);
    let out = sb.ok(&sb.root, &["burn", "kept", "--no-donate-cache"]);

    assert_eq!(out, "burned forest kept\n");
    assert!(!seed.join("build/new").exists());
    assert_eq!(ino(&seed.join("build/big")), big);
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
    let stamps = || -> Vec<_> {
        ["big", "small", "new", "state.lock"]
            .iter()
            .map(|file| {
                let file = tree.join("build").join(file);
                (ino(&file), modified(&file))
            })
            .collect()
    };
    let before = stamps();

    let out = sb.ok(&tree, &["cache", "donate"]);

    assert!(
        out.starts_with(
            "api\n  cache build: cloned to the seed: 4 files, 64.0 KiB hardlinked, 64.0 KiB \
             copied"
        ),
        "{out}"
    );
    // The tree's cache is just as it was, sharing its large file.
    assert_eq!(stamps(), before);
    let seed = sb.seed(&repo);
    assert!(same_file(&tree.join("build/big"), &seed.join("build/big")));
    assert!(!same_file(&tree.join("build/new"), &seed.join("build/new")));
    assert!(!same_file(
        &tree.join("build/state.lock"),
        &seed.join("build/state.lock")
    ));
    assert_eq!(
        modified(&seed.join("build/new")),
        modified(&tree.join("build/new"))
    );

    // Done once, there is nothing newer to give.
    let out = sb.ok(&tree, &["cache", "donate", "api"]);
    assert!(
        out.contains("nothing in it is newer than the seed's"),
        "{out}"
    );

    // The seed is now as the tree last gave it, so what the tree builds next
    // can be given too.
    write(&tree.join("build/later"), 10);
    let out = sb.ok(&tree, &["cache", "donate"]);
    assert!(
        out.contains("  cache build: cloned to the seed: 5 files"),
        "{out}"
    );
}

#[test]
fn a_seed_changed_since_a_tree_grafted_it_is_kept_unless_forced() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "first", "repos/api"]);
    sb.ok(&sb.root, &["new", "second", "repos/api"]);
    let second = sb.forest("second").join("api");
    write(&sb.forest("first").join("api/build/first"), 10);
    // The second tree builds later than the first.
    write(&second.join("build/second"), 10);
    set_modified(&second.join("build/second"), from_now(60));
    let out = sb.ok(&sb.root, &["burn", "first"]);
    assert!(out.contains("  cache build: moved to the seed: "), "{out}");

    // The second tree's cache is newer, but lacks what the first gave.
    let out = sb.ok(&second, &["cache", "donate"]);

    assert!(
        out.contains(
            "  cache build: the seed's changed since this tree grafted it, so donating would \
             lose that; --force donates anyway\n"
        ),
        "{out}"
    );
    let seed = sb.seed(&repo);
    assert!(seed.join("build/first").is_file());
    assert!(!seed.join("build/second").exists());

    let out = sb.ok(&second, &["cache", "donate", "--force"]);
    assert!(out.contains("  cache build: cloned to the seed: "), "{out}");
    assert!(seed.join("build/second").is_file());
    assert!(!seed.join("build/first").exists());
}

#[test]
fn the_seed_takes_the_main_checkouts_newer_output() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "early", "repos/api"]);
    // The main checkout builds after the seed took its cache.
    write(&repo.join("build/main"), 10);
    let main = snapshot(&repo);

    let out = sb.ok(&sb.root, &["new", "late", "repos/api"]);

    assert!(
        out.contains(
            "  cache build: the seed took the main checkout's newer cache: 3 files, 64.0 KiB \
             hardlinked, 20 B copied\n  cache build: grafted from the seed: 3 files, "
        ),
        "{out}"
    );
    assert!(sb.forest("late").join("api/build/main").is_file());
    assert_eq!(snapshot(&repo), main, "the main checkout changed");

    // A tree planted before then would lose that by donating.
    let early = sb.forest("early").join("api");
    write(&early.join("build/early"), 10);
    set_modified(&early.join("build/early"), from_now(60));
    let out = sb.ok(&early, &["cache", "donate"]);
    assert!(
        out.contains("the seed's changed since this tree grafted it"),
        "{out}"
    );
}

#[test]
fn cargos_profiles_and_targets_are_given_one_by_one() {
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
    sb.ok(&sb.root, &["new", "phone", "repos/game"]);
    assert!(
        sb.forest("desktop")
            .join(".workforest-grafts/game")
            .is_file()
    );
    // The desktop forest builds for the host; the phone forest for Android,
    // and then the host's build scripts, after the desktop's build.
    write(
        &sb.forest("desktop")
            .join("game/target/debug/deps/libnew.rlib"),
        10,
    );
    let phone = sb.forest("phone").join("game");
    write(&phone.join(format!("{android}/libnew.so")), 10);
    write(&phone.join("target/debug/build/script"), 10);
    set_modified(&phone.join("target/debug/build/script"), from_now(60));

    let out = sb.ok(&sb.root, &["cut", "game", "-f", "desktop"]);

    assert!(
        out.contains("  cache target: moved debug to the seed: 2 files, 64.0 KiB, newest "),
        "{out}"
    );
    assert!(
        !sb.forest("desktop")
            .join(".workforest-grafts/game")
            .exists()
    );

    let out = sb.ok(&sb.root, &["burn", "phone"]);

    assert!(
        out.contains(
            "  cache target: moved aarch64-linux-android to the seed: 2 files, 64.0 KiB, newest "
        ),
        "{out}"
    );
    assert!(
        out.contains("; kept the seed's debug, changed since this tree grafted it\n"),
        "{out}"
    );
    let seed = sb.seed(&repo);
    assert!(seed.join("target/debug/deps/libnew.rlib").is_file());
    assert!(!seed.join("target/debug/build/script").exists());
    assert!(seed.join(format!("{android}/libnew.so")).is_file());
    assert!(seed.join(format!("{android}/libold.so")).is_file());
    assert_eq!(sb.git(&repo, &["status", "--porcelain"]), "");
}

#[test]
fn a_cache_a_build_holds_stays_out_of_the_seed() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    write(&repo.join("build/debug/.cargo-lock"), 0);
    set_modified(&repo.join("build/debug/.cargo-lock"), 1_000_000_000);
    sb.ok(&sb.root, &["new", "busy", "repos/api"]);
    let tree = sb.forest("busy").join("api");
    write(&tree.join("build/debug/new"), 10);
    let seed = sb.seed(&repo);

    let lock = fs::File::open(tree.join("build/debug/.cargo-lock")).unwrap();
    lock.lock().unwrap();
    let out = sb.ok(&tree, &["cache", "donate", "--force"]);
    drop(lock);

    assert!(
        out.contains(
            "  cache build: a build holds debug/.cargo-lock in the tree, so its cache may be half \
             written; left alone\n"
        ),
        "{out}"
    );
    assert!(!seed.join("build/debug/new").exists());

    // A build in the main checkout keeps the seed from taking its cache, and
    // the tree grafts the seed's as it is.
    write(&repo.join("build/debug/main"), 10);
    let lock = fs::File::open(repo.join("build/debug/.cargo-lock")).unwrap();
    lock.lock().unwrap();
    let out = sb.ok(&sb.root, &["new", "meanwhile", "repos/api"]);
    drop(lock);

    assert!(
        out.contains(
            "  cache build: a build holds debug/.cargo-lock in the main checkout, so the seed \
             keeps what it has\n  cache build: grafted from the seed: "
        ),
        "{out}"
    );
    assert!(!seed.join("build/debug/main").exists());

    let out = sb.ok(&tree, &["cache", "donate"]);
    assert!(out.contains("  cache build: cloned to the seed"), "{out}");
}

#[test]
fn a_tree_on_another_filesystem_from_its_main_checkout_still_gets_a_seed() {
    let sb = Sandbox::new();
    let Some(elsewhere) = other_filesystem(&sb.root) else {
        eprintln!("skipped: /dev/shm is missing or on the same filesystem");
        return;
    };
    let repo = sb.stale_repo("api");
    let root = [("WORKFOREST_ROOT", elsewhere.path().as_os_str())];
    let out = sb.workforest_with(&sb.root, &["new", "far", "repos/api"], &root);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("  cache build: the seed has none; starting cold\n"),
        "{}",
        stdout(&out)
    );
    write(&elsewhere.path().join("far/api/build/new"), 10);

    let out = sb.workforest_with(&sb.root, &["burn", "far"], &root);

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("  cache build: moved to the seed: 1 file, 10 B, newest "),
        "{}",
        stdout(&out)
    );
    let out = sb.workforest_with(&sb.root, &["new", "near", "repos/api"], &root);
    assert!(
        stdout(&out).contains("  cache build: grafted from the seed: 1 file, "),
        "{}",
        stdout(&out)
    );
    assert!(elsewhere.path().join("near/api/build/new").is_file());
    assert!(
        sb.seed_in(elsewhere.path(), &repo)
            .join("build/new")
            .is_file()
    );
}

#[test]
fn what_an_interrupted_donation_left_is_cleared_away() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "again", "repos/api"]);
    let tree = sb.forest("again").join("api");
    write(&tree.join("build/new"), 10);
    let seed = sb.seed(&repo);
    write(&seed.join(".workforest-staging/outgoing/half"), 10);

    let out = sb.ok(&tree, &["cache", "donate"]);

    assert!(out.contains("  cache build: cloned to the seed"), "{out}");
    assert!(!seed.join(".workforest-staging").exists());
    assert!(seed.join("build/new").is_file());
}

#[test]
fn a_cold_tree_has_nothing_to_donate_until_it_builds() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\nlogs never\n");
    sb.publish(&repo, ".gitignore", "/build\n/logs\n");
    sb.ok(&sb.root, &["new", "cold", "repos/api", "--no-cache"]);
    let tree = sb.forest("cold").join("api");
    write(&tree.join("logs/today"), 10);

    let out = sb.ok(&tree, &["cache", "donate"]);

    assert_eq!(out, "api\n  cache build: the tree has none\n");
    assert!(!sb.seed(&repo).exists());

    // Built from cold, it gives the seed its first cache.
    write(&tree.join("build/out"), 10);
    let out = sb.ok(&tree, &["cache", "donate"]);
    assert!(
        out.contains(
            "  cache build: cloned to the seed: 1 file, 0 B hardlinked, 10 B copied, newest "
        ),
        "{out}"
    );
    assert!(out.contains("; the seed had none\n"), "{out}");
    assert!(!sb.seed(&repo).join("logs").exists());
}

#[test]
fn fire_donates_the_caches_of_the_trees_it_burns() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    let tree = sb.pull_request("pr", "api");
    write(&tree.join("build/new"), 10);
    sb.merge_on_remote("api", "pr", Merge::Squash);

    let out = sb.ok(&sb.root, &["fire", "--yes"]);

    assert!(
        out.contains("api: donating caches\n  cache build: moved to the seed: "),
        "{out}"
    );
    assert!(sb.seed(&repo).join("build/new").is_file());

    // Its JSON stays JSON.
    let tree = sb.pull_request("again", "api");
    write(&tree.join("build/newer"), 10);
    sb.merge_on_remote("api", "again", Merge::Squash);
    let out = sb.ok(&sb.root, &["fire", "--yes", "--json"]);
    let report: Value = serde_json::from_str(&out).unwrap_or_else(|err| panic!("{err}: {out}"));
    assert_eq!(report["forests"][0]["done"], true, "{report:#}");
    assert!(sb.seed(&repo).join("build/newer").is_file());
}

#[test]
fn cache_seeds_lists_the_seeds_and_unseed_deletes_them() {
    let sb = Sandbox::new();
    let api = sb.stale_repo("api");
    let web = sb.stale_repo("web");
    sb.ok(&sb.root, &["new", "both", "repos/api", "repos/web"]);
    sb.ok(&sb.root, &["burn", "both"]);
    let (api_seed, web_seed) = (sb.seed(&api), sb.seed(&web));
    let name = |seed: &Path| seed.file_name().unwrap().to_str().unwrap().to_owned();

    let out = sb.ok(&sb.root, &["cache", "seeds"]);

    let days = (from_now(0) - 1_000_000_000) / 86400;
    assert!(
        out.contains(&format!(
            "{}  {}\n  build                  2 files, 64.0 KiB, newest {days} days old\n",
            name(&api_seed),
            api.display()
        )),
        "{out}"
    );

    // A seed whose main checkout is gone.
    fs::rename(&web, sb.root.join("moved")).unwrap();
    let out = sb.ok(&sb.root, &["cache", "seeds"]);
    assert!(
        out.contains(&format!("{}  {} (gone)\n", name(&web_seed), web.display())),
        "{out}"
    );
    let out = sb.ok(&sb.root, &["cache", "seeds", "--json"]);
    let report: Value = serde_json::from_str(&out).unwrap_or_else(|err| panic!("{err}: {out}"));
    assert_eq!(report["seeds"][1]["gone"], true, "{report:#}");
    assert_eq!(report["seeds"][0]["caches"][0]["files"], 2, "{report:#}");

    let out = sb.ok(&sb.root, &["cache", "unseed", "--gone"]);
    assert_eq!(out, format!("removed seed {}\n", web_seed.display()));
    assert!(!web_seed.exists());
    sb.ok(&sb.root, &["cache", "unseed", "repos/api"]);
    assert!(!api_seed.exists());
    let err = sb.fails(&sb.root, &["cache", "unseed", "repos/api"]);
    assert!(err.contains("has no seed"), "{err}");
}

#[test]
fn cache_status_shows_how_old_each_cache_is_beside_the_seeds() {
    let sb = Sandbox::new();
    let repo = sb.stale_repo("api");
    sb.ok(&sb.root, &["new", "ages", "repos/api"]);
    let tree = sb.forest("ages").join("api");
    set_modified(&tree.join("build/small"), from_now(0) - 3 * 3600);

    let status = sb.ok(&sb.root, &["cache", "status", "ages"]);

    assert!(
        status.contains(", newest 3 h old (the seed's: "),
        "{status}"
    );
    let days = (from_now(0) - 1_000_000_000) / 86400;
    assert!(status.contains(&format!("{days} days old)\n")), "{status}");
    assert_eq!(modified(&repo.join("build/big")), at(1_000_000_000));
}
