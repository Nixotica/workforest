#!/bin/sh
# Install the workforest CLI from a GitHub release: download the tarball for
# this machine, check it against the release's SHA256SUMS, and install
# `workforest` and `wf` into ~/.local/bin, with shell completions and the agent
# skill under ~/.local/share.
#
#   curl -fsSL https://raw.githubusercontent.com/Nixotica/workforest/release/install.sh | sh
#
# Environment:
#   WORKFOREST_VERSION   the version to install, such as 0.14.0 [default: the
#                        latest release]
#   WORKFOREST_PREFIX    where to install [default: ~/.local]
#   WORKFOREST_RELEASES  where to download the release's files from, for
#                        testing [default: the GitHub release]
set -eu

repo=Nixotica/workforest
prefix=${WORKFOREST_PREFIX:-$HOME/.local}

say() { printf 'workforest install: %s\n' "$*" >&2; }
fail() {
  say "$*"
  exit 1
}

# download <url> <file>
download() {
  if command -v curl >/dev/null 2>&1; then
    curl --fail --silent --show-error --location --output "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget --quiet --output-document="$2" "$1"
  else
    fail "needs curl or wget to download the release"
  fi
}

case "$(uname -s)/$(uname -m)" in
  Linux/x86_64 | Linux/amd64) target=x86_64-unknown-linux-musl ;;
  Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-musl ;;
  Darwin/arm64) target=aarch64-apple-darwin ;;
  *) fail "no prebuilt binary for $(uname -s) $(uname -m); see https://github.com/$repo#install" ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

version=${WORKFOREST_VERSION:-}
if [ -z "$version" ]; then
  download "https://api.github.com/repos/$repo/releases/latest" "$tmp/latest.json" ||
    fail "could not ask GitHub for the latest release; set WORKFOREST_VERSION"
  version=$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "$tmp/latest.json" | head -n 1)
  [ -n "$version" ] || fail "could not read the latest release's version"
fi
version=${version#v}
releases=${WORKFOREST_RELEASES:-https://github.com/$repo/releases/download/v$version}
name="workforest-$version-$target"

download "$releases/$name.tar.gz" "$tmp/$name.tar.gz" ||
  fail "release $version has no binary for $target; see https://github.com/$repo#install"
download "$releases/SHA256SUMS" "$tmp/SHA256SUMS" ||
  fail "release $version has no SHA256SUMS to check the download against"
expected=$(awk -v file="$name.tar.gz" '$2 == file || $2 == "*" file { print $1 }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || fail "SHA256SUMS doesn't list $name.tar.gz"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$name.tar.gz" | cut -d ' ' -f 1)
elif command -v shasum >/dev/null 2>&1; then
  actual=$(shasum -a 256 "$tmp/$name.tar.gz" | cut -d ' ' -f 1)
else
  fail "needs sha256sum or shasum to check the download"
fi
[ "$actual" = "$expected" ] || fail "$name.tar.gz doesn't match its checksum in SHA256SUMS"

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$prefix/bin"
cp "$tmp/$name/bin/workforest" "$prefix/bin/workforest.new"
chmod 755 "$prefix/bin/workforest.new"
mv -f "$prefix/bin/workforest.new" "$prefix/bin/workforest"
ln -sf workforest "$prefix/bin/wf"
(cd "$tmp/$name/share" && find . -type f) | while IFS= read -r file; do
  mkdir -p "$prefix/share/$(dirname "$file")"
  cp "$tmp/$name/share/$file" "$prefix/share/$file"
done
say "installed $("$prefix/bin/workforest" --version) as $prefix/bin/workforest and wf"
case ":$PATH:" in
  *":$prefix/bin:"*) ;;
  *) say "$prefix/bin isn't on your PATH yet: add it in your shell's startup file" ;;
esac

# Once a person can answer, say where their repos live so they can be named.
if [ -t 1 ] && (: </dev/tty) 2>/dev/null &&
  "$prefix/bin/workforest" config 2>/dev/null | grep -q 'repos .*not set'; then
  "$prefix/bin/workforest" setup </dev/tty || say "run \`workforest setup\` later to name repos"
fi
