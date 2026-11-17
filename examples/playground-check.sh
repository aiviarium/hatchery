#!/usr/bin/env bash
# Runs the steps of docs/playground.md in a playground that
# examples/playground.sh made, and fails when one behaves differently.
#
#   examples/playground-check.sh [DIR]
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dir=${1:-$root/target/playground}
# shellcheck disable=SC1091
. "$dir/env.sh"

crow=(--target crow --fact crow=1.2.0)
rookery=(--target rookery --fact os=linux)

says() { # says TEXT COMMAND…: the command succeeds and prints TEXT
  local want=$1 out
  shift
  out=$("$@" 2>&1) || { echo "FAILED: $*"; echo "$out"; exit 1; }
  grep -qF -- "$want" <<<"$out" || { echo "MISSING '$want': $*"; echo "$out"; exit 1; }
  echo "ok: $*"
}

refuses() { # refuses TEXT COMMAND…: the command fails and prints TEXT
  local want=$1 out
  shift
  if out=$("$@" 2>&1); then echo "SUCCEEDED: $*"; echo "$out"; exit 1; fi
  grep -qF -- "$want" <<<"$out" || { echo "MISSING '$want': $*"; echo "$out"; exit 1; }
  echo "ok, refused: $*"
}

rm -rf plugins host offline fresh oneoff strict strict.kdl

says "example/toolbox  1.1.0" hatchery list --offered "${crow[@]}"
says "installed example/toolbox 1.0.0" hatchery add example/toolbox@1.0.0 "${crow[@]}" --dir plugins
test -f plugins/example/hello/crow/crow.json
says "1.0.0      1.1.0" hatchery list --dir plugins --offered "${crow[@]}"
says "updated example/toolbox 1.0.0 -> 1.1.0" hatchery update --dir plugins "${crow[@]}"
refuses "required by example/toolbox" hatchery remove example/hello --dir plugins

refuses "requires rookery >=0.6" hatchery add example/toolbox "${rookery[@]}" --fact rookery=0.5.0 --dir host
says "installed example/toolbox 1.1.0" hatchery add example/toolbox "${rookery[@]}" --fact rookery=0.7.0 --dir host
test -f host/example/toolbox/rookery/tasks.txt

mkdir fresh && cp plugins/eggs.lock fresh/
says "installed example/toolbox 1.1.0" hatchery install --dir fresh "${crow[@]}"

says "toolbox-1.1.0.egg" hatchery fetch example/toolbox@1.1.0 --fat --out offline
says "(fat egg)" hatchery inspect offline/toolbox-1.1.0.egg
says "installed example/hello" hatchery add --git "$dir/aviary.git" --path packages/hello --ref main "${crow[@]}" --dir oneoff

# Without unsigned-tags, a registry with a key installs only signed eggs.
sed 's/ unsigned-tags=#true//' hatchery.kdl > strict.kdl
refuses "only a tag" hatchery --config strict.kdl add example/hello "${crow[@]}" --dir strict
says "installed example/toolbox 1.1.0" hatchery --config strict.kdl add example/toolbox --target rookery --fact os=linux --fact rookery=0.7.0 --dir strict
rm -rf strict strict.kdl

# A tag that moves after it was locked is refused.
git -C aviary commit --quiet --allow-empty -m "Not the release"
git -C aviary tag -f v1.1.0 >/dev/null
git -C aviary push --quiet --force origin v1.1.0
rm -rf fresh && mkdir fresh && cp plugins/eggs.lock fresh/
refuses "v1.1.0" hatchery install --dir fresh "${crow[@]}"

echo "The playground behaves as documented."
