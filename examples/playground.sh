#!/usr/bin/env bash
# Sets up a folder to try hatchery in: a git repository of example packages
# with two released versions and built, signed eggs, and a configuration that
# lists it as a registry. Nothing outside the folder is touched.
#
#   examples/playground.sh [DIR]      DIR is target/playground by default
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
dir=${1:-$root/target/playground}

command -v git >/dev/null || { echo "error: git is not installed" >&2; exit 1; }

echo "Building hatchery"
cargo build --quiet --locked --manifest-path "$root/Cargo.toml" -p hatchery-cli
bin=$root/target/debug

rm -rf "$dir"
mkdir -p "$dir/bin"
dir=$(cd "$dir" && pwd)
ln -s "$bin/hatchery" "$dir/bin/hatchery"

# The playground's own configuration, cache and git identity.
export PATH="$dir/bin:$PATH"
export HATCHERY_CONFIG="$dir/hatchery.kdl"
export HATCHERY_CACHE="$dir/cache"
export GIT_CONFIG_GLOBAL="$dir/gitconfig"
export GIT_CONFIG_NOSYSTEM=1
git config --global user.name playground
git config --global user.email playground@example.org
git config --global init.defaultBranch main
git config --global advice.detachedHead false

# The "hosted" repository and a checkout of it.
git init --quiet --bare "$dir/aviary.git"
cp -r "$here/aviary" "$dir/aviary"
cd "$dir/aviary"
git init --quiet
git remote add origin "$dir/aviary.git"

keys=$(hatchery keygen 2>/dev/null)
HATCHERY_SIGN_KEY=$(sed -n 1p <<<"$keys")
public=$(sed -n 2p <<<"$keys")
export HATCHERY_SIGN_KEY
printf '%s\n' "$HATCHERY_SIGN_KEY" > "$dir/sign.key"

# A release: commit, tag, then pack and publish the eggs of the package that
# has a build. `hello` has none, so its tag is its release.
release() {
  local version=$1
  sed -i.bak "s/version \"[0-9.]*\"/version \"$version\"/" packages.kdl && rm packages.kdl.bak
  git add -A
  git commit --quiet -m "Release $version"
  git tag "v$version"
  git push --quiet origin main "v$version"
  rm -rf dist
  hatchery pack packages/toolbox --out dist >/dev/null
  hatchery publish dist/*.egg --sign-key-env HATCHERY_SIGN_KEY >/dev/null
  echo "Released $version"
}

release 1.0.0
echo '{ "name": "hello", "greeting": "Hello again from an egg" }' > packages/hello/crow/crow.json
release 1.1.0

cat > "$dir/hatchery.kdl" <<EOF
// The registries this playground searches. A folder counts as local, so new
// versions are not held back. Add min-age="3d" to the line to see the hold.
// With a key, only signed eggs install. hello has no build and so no egg:
// its tag is its release, and unsigned-tags=#true lets it install from it.
registries {
  git "$dir/aviary.git" key="$public" unsigned-tags=#true
}
EOF

cat > "$dir/env.sh" <<EOF
# Source this file: . $dir/env.sh
export PATH="$dir/bin:\$PATH"
export HATCHERY_CONFIG="$dir/hatchery.kdl"
export HATCHERY_CACHE="$dir/cache"
export GIT_CONFIG_GLOBAL="$dir/gitconfig"
export GIT_CONFIG_NOSYSTEM=1
export HATCHERY_SIGN_KEY="\$(cat "$dir/sign.key")"
cd "$dir"
EOF

cat <<EOF

The playground is in $dir

  . $dir/env.sh

Then see docs/playground.md for what to try. To start:

  hatchery list --offered --target crow --fact crow=1.2.0
  hatchery add example/toolbox@1.0.0 --target crow --fact crow=1.2.0 --dir plugins
EOF
