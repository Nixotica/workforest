//! Cargo's `target` directory, grafted without a declaration.
//!
//! TODO: re-assess once Cargo's cross-workspace build cache lands, a 2026 Rust
//! project goal: <https://goals.rust-lang.org/2026/cargo-cross-workspace-cache.html>.
//! If Cargo shares built units between workspaces itself, grafting `target`
//! may become unnecessary, or should defer to Cargo's cache.

use super::ecosystem::Ecosystem;

/// Cargo's `target` directory. Artifacts are named by a hash of the package
/// and its path relative to the workspace root, so they still match in a tree
/// at another path.
///
/// The always-copy globs are a best guess, not a proof: the files Cargo 1.94
/// was seen to rewrite in place or lock, rather than replace, under `cache
/// doctor` with every other file hardlinked and under strace, across fresh
/// builds, rebuilds after edits, build-script reruns, test builds, `cargo
/// check`, `cargo clippy` and `cargo doc`. The size split still covers small
/// files nobody has seen written.
pub const CARGO: Ecosystem = Ecosystem {
    name: "cargo",
    marker: "Cargo.toml",
    path: "target",
    always_copy: &[
        // A lock belongs to the inode, so a shared one makes builds in every
        // tree wait for each other: Cargo's build lock, and rustc's
        // incremental session locks.
        ".cargo-lock",
        "*.lock",
        // Cargo's record of what is up to date. Shared, a build in one tree
        // makes another take its stale output for fresh.
        ".fingerprint/*",
        "invoked.timestamp",
        // rustc rewrites dep-info in place.
        "*.d",
        // A build script's results: the code it generated, and what it
        // printed, which Cargo replays without rerunning it. Build scripts
        // rerun in a tree whose sources are newer than the cache, and rewrite
        // these in place; some are large, such as aws-lc-sys's `output`.
        "build/*/out/*",
        "build/*/output",
        "build/*/root-output",
        "build/*/stderr",
        // Cargo's cache of what the compiler reports about itself, rewritten
        // when `cargo clippy` and a build take turns.
        ".rustc_info.json",
        // rustdoc rewrites its static files and its cross-crate indexes, such
        // as `trait.impl/**/*.js`, in place: shared, a tree's docs would leak
        // into the main checkout's.
        "doc/*",
    ],
    inert: &[
        // rustc's dep-info for units the tree doesn't rebuild: Cargo judges
        // freshness by its own record under `.fingerprint`, whose paths are
        // relative to the package.
        "*.d",
        // The out dir a build script ran with, which Cargo reads to replace
        // that path with the tree's when it replays the script's output.
        "build/*/root-output",
        // Superseded incremental sessions: rustc starts a new session in the
        // tree, and leaves these unread until it deletes them.
        "incremental/*",
        // Object files rustc keeps beside a debug build for debuggers, as it
        // does by default on macOS (split-debuginfo=unpacked). Their debug
        // info records the source paths they were compiled from, but no
        // build reads them back: a debugger may show the main checkout's
        // paths for code the tree reused from its cache, and that's all.
        "deps/*.o",
    ],
    build: "cargo build --all-targets && cargo doc",
};
