#!/usr/bin/env bash
# Cut a release: bump the version everywhere, check, commit, tag, push, and
# publish the GitHub release from the changelog.
#
#   scripts/release.sh 0.1.1
#
# Requires a clean tree on main, a "## <version> (YYYY-MM-DD)" heading in
# CHANGELOG.md,
# cargo, gh (logged in), and the tools make check needs.
set -euo pipefail

version="${1:-}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: scripts/release.sh X.Y.Z" >&2
  exit 2
fi
cd "$(dirname "$0")/.."

branch="$(git rev-parse --abbrev-ref HEAD)"
[[ "$branch" == "main" ]] || { echo "on $branch, not main" >&2; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo "working tree is not clean" >&2; exit 1; }
git fetch -q origin main
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || { echo "main is not in sync with origin" >&2; exit 1; }
! git rev-parse -q --verify "refs/tags/v$version" >/dev/null || { echo "tag v$version exists" >&2; exit 1; }
grep -Eq "^## ${version//./\\.} \([0-9]{4}-[0-9]{2}-[0-9]{2}\)$" CHANGELOG.md || { echo "CHANGELOG.md has no '## $version (YYYY-MM-DD)' section" >&2; exit 1; }

# Checks run on the clean tree first; they do not depend on the version.
make check

# Version lives in two manifests plus the lockfile. Until the commit lands,
# any exit restores them so a rerun starts from a clean tree.
trap 'git checkout -- Cargo.toml Cargo.lock herdr-plugin.toml' EXIT
# First "version =" line only; BSD sed has no 0,/re/ address, so perl.
perl -pi -e 'BEGIN { $v = shift } if (!$done && s/^version = "[^"]+"/version = "$v"/) { $done = 1 }' "$version" Cargo.toml
perl -pi -e 'BEGIN { $v = shift } s/^version = "[^"]+"/version = "$v"/' "$version" herdr-plugin.toml
cargo update -q --workspace
grep -q "^version = \"$version\"" Cargo.toml herdr-plugin.toml
grep -A1 '^name = "goat-herdr"' Cargo.lock | grep -q "version = \"$version\""

cargo build --release

git add Cargo.toml Cargo.lock herdr-plugin.toml
git commit -q -m "Release $version"
trap - EXIT
git tag -a "v$version" -m "goat-herdr $version"
git push -q origin main "v$version"

# The section's lines, up to the next "## " heading or the end of the file.
notes="$(awk -v h="## $version (" 'index($0, h) == 1 { on = 1; next } on && /^## / { exit } on' CHANGELOG.md)"
gh release create "v$version" --title "$version" --notes "$notes"
echo "released v$version"
