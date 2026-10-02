#!/usr/bin/env bash
# Exercise an installed workforest the way a user would: plant a forest in a
# throwaway set of repos, check that unlanded work blocks the burn, land it,
# burn the forest, and install the skill.
#
# usage: ci/smoke.sh <prefix>   where <prefix>/bin/workforest is the install
set -euo pipefail

prefix=$(cd "${1:?usage: smoke.sh <install prefix>}" && pwd)
wf="$prefix/bin/workforest"
skill="$(cd "$(dirname "$0")/.." && pwd)/plugin/skills/workforest/SKILL.md"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HOME="$tmp/home" WORKFOREST_ROOT="$tmp/forests" WORKFOREST_REPOS="$tmp/repos"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$tmp/home/.gitconfig"
export GIT_AUTHOR_NAME=smoke GIT_AUTHOR_EMAIL=smoke@example.com
export GIT_COMMITTER_NAME=smoke GIT_COMMITTER_EMAIL=smoke@example.com
mkdir -p "$HOME" "$WORKFOREST_REPOS"

git init --quiet --initial-branch=main "$tmp/seed"
git -C "$tmp/seed" commit --quiet --allow-empty --message initial
git clone --quiet --bare "$tmp/seed" "$tmp/origin.git"
git clone --quiet "$tmp/origin.git" "$WORKFOREST_REPOS/demo"

"$wf" --version
"$wf" new smoke demo
tree="$("$wf" path smoke)/demo"
echo change >"$tree/file"
"$wf" status smoke | grep --quiet dirty
if "$wf" rm smoke; then
  echo "rm burned a forest with uncommitted work" >&2
  exit 1
fi
git -C "$tree" add file
git -C "$tree" commit --quiet --message change
git -C "$tree" push --quiet --set-upstream origin HEAD
"$wf" rm smoke --delete-branches
test ! -e "$WORKFOREST_ROOT/smoke"

cmp "$prefix/share/workforest/skills/workforest/SKILL.md" "$skill"
"$wf" skill install --dir "$tmp/skills"
cmp "$tmp/skills/workforest/SKILL.md" "$skill"
echo "smoke test passed"
