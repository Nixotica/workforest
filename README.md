# workforest

Isolated git worktrees for every piece of work. A **forest** is a directory
holding one git worktree ("tree") per repo involved in a piece of work, all on
one branch named after the forest. The repos' main checkouts are never touched,
so several pieces of work, or several coding agents, can be in flight at once.

```sh
workforest new auth-migration ~/code/api ~/code/web   # both trees on branch auth-migration
cd "$(workforest path auth-migration)"
workforest status                                     # what's dirty, what's ahead of its base
workforest burn auth-migration                        # refuses while any work would be lost
```

A forest with a single tree is normal. Trees are `git worktree`s, so they share
their repo's object store: a tree costs its checked-out files, not a copy of
the history.

## Install

workforest needs `git` on your `PATH`. Releases are rolling: every commit on
`main` that passes CI is released to the `release` branch, which each install
method below follows.

With Nix:

```sh
nix profile add github:Nixotica/workforest/release   # older Nix: nix profile install
```

or as a flake input, `workforest.url = "github:Nixotica/workforest/release";`,
adding `workforest.packages.${system}.default` to your packages. Update with
`nix profile upgrade` or `nix flake update workforest`.

With Cargo:

```sh
cargo install --locked --git https://github.com/Nixotica/workforest --branch release
```

## The agent skill

workforest ships a skill that teaches coding agents when to start a forest and
how to work in one. The skill doesn't need the CLI installed first: when it's
missing, the agent offers to install it.

As a Claude Code plugin, which follows the rolling releases:

```
/plugin marketplace add Nixotica/workforest#release
/plugin install workforest@workforest
```

`#release` makes the marketplace follow the `release` branch, which only ever
holds commits that passed CI. Without it, you follow `main`.

The plugin checks for the CLI whenever a session starts. If the CLI is missing
or too old, Claude Code says so as soon as the session opens, and Claude offers
to install it in its first reply. A plugin installed mid-session isn't loaded
until you run `/reload-plugins`, and reloading doesn't count as a session start.
The first check comes in your next session, or when you run `/clear`.

Claude Code leaves auto-update off for third-party marketplaces. To receive
each release automatically, turn it on under `/plugin` → **Marketplaces** →
workforest → **Enable auto-update**; otherwise run
`/plugin marketplace update workforest` to update.

Without the plugin, link the copy the Nix package installs, so the skill
updates along with the CLI:

```sh
ln -s ~/.nix-profile/share/workforest/skills/workforest ~/.claude/skills/workforest
```

or fetch it from the `release` branch, which takes a re-run to update:

```sh
mkdir -p ~/.claude/skills/workforest
curl -fsSL https://raw.githubusercontent.com/Nixotica/workforest/release/plugin/skills/workforest/SKILL.md \
  -o ~/.claude/skills/workforest/SKILL.md
```

For other agents, put the skill wherever they read skills from.

## Usage

| command | what it does |
| --- | --- |
| `workforest new <forest> [repo path...]` | start a forest, optionally planting trees right away |
| `workforest plant <repo path>...` | add trees to a forest (alias: `add`) |
| `workforest cut <tree>...` | remove trees from a forest, by name (alias: `remove`) |
| `workforest burn [forest]` | remove a forest and every tree in it (aliases: `rm`, `delete`) |
| `workforest fire` | burn every forest whose work has landed; a dry run unless `--yes` |
| `workforest ls [forest]` | list forests, or the trees in one |
| `workforest status [forest]` | per-tree branch, clean/dirty/landed, ahead/behind its base, pushed or not |
| `workforest path [forest]` | print a forest's path |
| `workforest config` | each setting, its value, and where it comes from |
| `workforest cache status [forest]` | per tree, how much of each build cache is still hardlinked to the main checkout |
| `workforest cache graft [tree...]` | graft build caches into trees already planted (`--force` replaces them) |
| `workforest cache drop [tree...]` | delete trees' grafted build caches |
| `workforest cache paths <repo path>` | the build caches a repo declares, and where each is declared |
| `workforest cache doctor <repo path>` | test that a repo's build caches are safe to graft |

`wf` is short for `workforest`: the Nix package installs it as a symlink. With
cargo, add it yourself: `ln -s workforest ~/.cargo/bin/wf`.

Inside a forest, commands act on that forest unless told otherwise. Every tree
gets the branch named after its forest unless you pass `--branch`, and branches
off `origin/HEAD` (else `main` or `master`) unless you pass `--base`. `cut`
and `burn` refuse to delete uncommitted changes, or commits that are neither
pushed nor landed on the base, unless you pass `--force`. Work has landed once
everything its branch changed is on the base, so a regular, squash or rebase
merge all count, even after the merged branch is deleted. A squash merge still
counts once the base has rewritten lines next to it, as lockfiles see all the
time, unless a later commit reverts it. Once the base branch
itself is gone, as a stacked branch's base is after it merges and the stacked
pull request is retargeted, the repo's default branch (`origin/HEAD`) stands in
for it. A base that lives on, such as a release branch, never has a stand-in.
They check the base as you last fetched it: fetch with `--prune` after merging,
then burn. Run `workforest help <command>` for the details.

Forests pile up, and `workforest fire` clears them all at once. It fetches the
remotes the trees' bases are on, pruning deleted branches, then calls a tree
dead when `burn` would accept it and its branch has commits of its own that
have all landed on its base, or on the default branch standing in for a base
that is gone, or when its directory is gone. A tree with uncommitted changes,
with commits not on its base, or with nothing committed yet is live, and one
that git can't judge, such as one whose base was deleted before its work
landed anywhere, is left alone. A forest burns
when every tree in it is dead: `fire` lists each such forest and why, and
`fire --yes` burns them. `--scorch` also cuts dead trees out of forests that are
still live, and `--delete-branches` deletes the dead trees' branches, keeping
any with commits not on its base. `fire` never removes anything `burn` would
refuse, and has no `--force`.

Repos are given as paths, absolute or relative to the current directory:
`wf new fix-login .` starts a forest for the repo you're in. workforest assumes
nothing about where your repos live, so a bare name like `api` is rejected;
#20 tracks naming repos after a one-time setup. Forests live under the forest
root, `~/.workforest` unless [configured](#configuration) otherwise.

## Build caches

A fresh tree has no build output, so its first build would start from cold.
Instead, `new` and `plant` graft each repo's build cache from its main checkout
into the tree. A repo declares its cache directories in `.workforest-cache` at
its root, committed with it, or in `workforest-cache` in its git common dir
(usually `.git/workforest-cache`), which is machine-local and overrides the
committed file path by path:

```
# path   mode    always-copy globs
build    clone   *.lock,state/*
.venv    never
```

| mode | what each tree gets |
| --- | --- |
| `clone` (default) | the main checkout's directory, with files of 64 KiB and up hardlinked, costing no disk, and smaller files copied |
| `never` | nothing; the cache starts cold |

Cargo's `target` needs no declaration: a repo with a `Cargo.toml` at its root
grafts it as `clone`, and says nothing when its main checkout has no `target`,
as when it builds into a shared `CARGO_TARGET_DIR`. Its always-copy globs are a
best guess at every file Cargo rewrites in place or locks: lock files,
fingerprints, dep-info, `.rustc_info.json`, build scripts' results
(`build/*/out/*`, `build/*/output` and the like), which build scripts rewrite
when they rerun and which can be large, and `doc/`, whose static files and
cross-crate indexes rustdoc rewrites. Every dependency's build-script output is
copied, though only the workspace's own build scripts rerun in a tree, so
crates that build C libraries can make a graft's copies large. Dependencies
stay warm in the tree, since Cargo keys their freshness on their version; the
workspace's own crates rebuild once, since a fresh checkout's sources are newer
than the grafted output. A declaration for `target` overrides the built-in
entry, and keeps its globs unless it lists its own: `target never` leaves it
cold. Don't point trees at a shared `CARGO_TARGET_DIR` instead: Cargo's
artifact names don't include the workspace path, so trees on different
branches would build over each other's output. Cargo's own cross-workspace
build cache, a [2026 project goal](https://goals.rust-lang.org/2026/cargo-cross-workspace-cache.html),
may make grafting `target` unnecessary; this is to be re-assessed when it
lands.

The size split is a bet on how build tools behave, not a guarantee. They
replace large artifacts wholesale when they rebuild them, so sharing those is
free, while the files they rewrite in place (fingerprints, dep-info,
timestamps, locks) are usually small and get private copies. But a hardlink
shares the file itself, so a large file that a build rewrites in place reaches
every checkout that shares it. List each one as an always-copy glob, and use
`cache doctor` to find them. A glob containing `/` matches the path inside the
cache at any depth, any other glob the file name, and `-` lists none. `#`
starts a comment anywhere on a line, so neither a path nor a glob can contain
one. Copies keep their modification times, so a build tool still sees the
tree's freshly checked-out sources as newer than the grafted output, and
rebuilds what they changed. A tree planted earlier may have files older than
the main checkout's last build, so `cache graft` marks those as changed,
setting their modification time to now: otherwise the grafted output would
pass as built from them.

A grafted cache is a snapshot of the main checkout's when the tree was
planted. A build replaces the large files it rebuilds rather than rewriting
them, which is what `cache doctor` checks, so a build in either checkout breaks
those files' hardlinks: the two drift apart without either changing the other;
`workforest cache status` shows how much each tree still shares. The graft is
only as warm as the main checkout's last build: one built long ago grafts
fine, but leaves the tree more to rebuild.

workforest only grafts into, replaces or deletes a cache path that git ignores
and tracks nothing under, so a mistaken declaration can't touch source. A cache
whose main checkout is on another filesystem is left cold, since hardlinks
can't cross filesystems. A graft is cloned beside the cache and moved into
place, so an interrupted one never leaves half a cache behind, and `cache graft
--force` only gives up a tree's cache once it has a new one to put there.

Before trusting a `clone` entry, test it:

```sh
workforest cache doctor ~/code/api --cmd 'make'
```

In a cargo repo, `--cmd` defaults to `cargo build --all-targets && cargo doc`.
The doctor grafts the caches into a throwaway worktree, builds there, and fails
if the build wrote through a hardlink to the main checkout's cache, changing a
file's contents, times or permissions. It also fails if the build failed or
changed nothing in the grafted cache, since either proves nothing. It can't
prove that a cache works at another path, which takes knowing what the build
reads, so it lists the grafted files that name the main checkout's path: that
is how a cache records where it was built. In a cargo repo it leaves out the
files that name the main checkout without tying the tree's build to it: rustc's
dep-info, build scripts' `root-output`, and superseded incremental sessions. Known not to work at another path: a Python `.venv` (its
scripts record the path they were created at), a CMake `build/`
(`CMakeCache.txt` records the source directory), and Gradle's in-project
`.gradle/`. An interrupted doctor leaves its scratch worktree, a `.doctor-*`
directory under the forest root, until the next doctor of that repo removes it.

`--no-cache` on `new` and `plant` skips the graft, and `cache.link_min`
(default `65536`, see [Configuration](#configuration)) sets the size from which
files are hardlinked. On btrfs and XFS a copy shares its data until written, so it
costs little disk either.

## Configuration

Settings live in `$XDG_CONFIG_HOME/workforest/config.toml`, which is
`~/.config/workforest/config.toml` unless `XDG_CONFIG_HOME` is set, on macOS
too. The file is optional, and every setting in it is too:

```toml
forest_root = "~/.workforest"   # where forests live

[cache]
link_min = 65536                # bytes from which grafted cache files are hardlinked
```

An environment variable overrides each setting: `WORKFOREST_ROOT` and
`WORKFOREST_CACHE_LINK_MIN`. So a setting comes from its environment variable,
else the config file, else its default. Paths in the file are absolute or start
with `~/`. `workforest config` prints each setting's value and where it came
from, and warns about settings it doesn't know, such as a misspelt one. A file
that isn't valid TOML stops every command; a bad `cache.link_min` stops only
the commands that graft.

## Development

```sh
nix develop        # cargo, clippy, rustfmt, rust-analyzer and git
nix flake check    # tests, clippy, rustfmt and the package build, as CI runs them
```

When CI passes on a commit pushed to `main`, the release workflow moves the
`release` branch forward to it. Bump `version` in `Cargo.toml` when the
command-line interface changes: the first commit to pass CI with a new version
is tagged `v<version>` and gets a GitHub release.

## License

MIT
