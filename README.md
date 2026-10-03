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
| `workforest ls [forest]` | list forests, or the trees in one |
| `workforest status [forest]` | per-tree branch, clean/dirty/landed, ahead/behind its base, pushed or not |
| `workforest path [forest]` | print a forest's path |
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
merge all count, even after the merged branch is deleted. They check the base
as you last fetched it: fetch after merging, then burn. Run
`workforest help <command>` for the details.

Repos are given as paths, absolute or relative to the current directory:
`wf new fix-login .` starts a forest for the repo you're in. workforest assumes
nothing about where your repos live, so a bare name like `api` is rejected;
#20 tracks naming repos after a one-time setup. Forests live under
`$WORKFOREST_ROOT` (default `~/.workforest`).

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
| `share` | a symlink to one directory under `$WORKFOREST_CACHE` that every tree of the repo shares; only for content-addressed caches that tolerate several builds writing at once |
| `never` | nothing; the cache starts cold |

The size split is what makes `clone` safe. Build tools replace large artifacts
wholesale when they rebuild them, so sharing those is free, while the files
they rewrite in place (fingerprints, dep-info, timestamps, locks) are small and
get private copies. List any large file that is rewritten in place as an
always-copy glob: a glob containing `/` matches the path inside the cache at
any depth, any other glob the file name, and `-` lists none. Copies keep their
modification times, so a build tool still sees the tree's freshly checked-out
sources as newer than the grafted output, and rebuilds what they changed.

The split is a best guess, not a guarantee. A hardlink shares the file itself,
so whatever a build rewrites in place rather than replacing reaches every
checkout that shares it; small files are just where such state usually lives.
A large file that is rewritten in place needs an always-copy glob, and `cache
doctor` is how to find one.

A grafted cache is a snapshot of the main checkout's when the tree was
planted. A build replaces the large files it rebuilds rather than rewriting
them, which is what `cache doctor` checks, so a build in either checkout breaks
those files' hardlinks: the two drift apart without either changing the other;
`workforest cache status` shows how much each tree still shares. The graft is
only as warm as the main checkout's last build: one built long ago grafts
fine, but leaves the tree more to rebuild.

workforest only grafts into, replaces or deletes a cache path that git ignores
and tracks nothing under, so a mistaken declaration can't touch source. A
`share` symlink needs a `.gitignore` entry without a trailing slash, since git
doesn't count a symlink as a directory. A cache whose main checkout is on
another filesystem is left cold, since hardlinks can't cross filesystems.

Before trusting a `clone` entry, test it:

```sh
workforest cache doctor ~/code/api --cmd 'make'
```

It grafts the caches into a throwaway worktree, builds there, and fails if any
file in the main checkout's cache changed. Known to fail: a Python `.venv` (its
scripts record the path they were created at), a CMake `build/`
(`CMakeCache.txt` records the source directory), and Gradle's in-project
`.gradle/`.

`--no-cache` on `new` and `plant` skips the graft. `$WORKFOREST_CACHE`
(default `$XDG_CACHE_HOME/workforest`, else `~/.cache/workforest`) holds
`share` caches, and `$WORKFOREST_CACHE_LINK_MIN` (default `65536`) sets the
size from which files are hardlinked.

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
