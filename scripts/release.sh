#!/usr/bin/env bash
# Cut a release: bump the version everywhere, check, commit, tag, push, and
# publish the GitHub release from the changelog.
#
#   scripts/release.sh 0.1.1
#
# Requires a clean tree on main, a "## <version> (" heading in CHANGELOG.md,
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
grep -q "^## $version (" CHANGELOG.md || { echo "CHANGELOG.md has no '## $version (' section" >&2; exit 1; }

# Version lives in two manifests plus the lockfile.
# First "version =" line only; BSD sed has no 0,/re/ address, so perl.
perl -pi -e 'BEGIN { $v = shift } if (!$done && s/^version = "[^"]+"/version = "$v"/) { $done = 1 }' "$version" Cargo.toml
perl -pi -e 'BEGIN { $v = shift } s/^version = "[^"]+"/version = "$v"/' "$version" herdr-plugin.toml
cargo update -q --workspace
grep -q "^version = \"$version\"" Cargo.toml herdr-plugin.toml
grep -A1 '^name = "goat-herdr"' Cargo.lock | grep -q "version = \"$version\""

make check
cargo build --release

git add Cargo.toml Cargo.lock herdr-plugin.toml
git commit -q -m "Release $version"
git tag -a "v$version" -m "goat-herdr $version"
git push -q origin main "v$version"

notes="$(sed -n "/^## $version (/,/^## /p" CHANGELOG.md | sed '1d;$d')"
gh release create "v$version" --title "$version" --notes "$notes"
echo "released v$version"
