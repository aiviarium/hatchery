#!/usr/bin/env bash
# The checks CI runs, by name. With no argument, all of them.
#
#   ci/checks.sh [fmt] [clippy] [test] [doc] [vale] [typos] [deny] [links]
#
# CI installs every tool. On your machine a check whose tool is missing is
# skipped with a note, unless CI=true.
set -euo pipefail
cd "$(dirname "$0")/.."

prose=(README.md AGENTS.md CLAUDE.md docs .github/CONTRIBUTING.md .github/SECURITY.md .github/CODE_OF_CONDUCT.md)

need() {
  command -v "$1" >/dev/null && return 0
  if [ "${CI:-}" = true ]; then
    echo "error: $1 is not installed" >&2
    exit 1
  fi
  echo "skipped: $1 is not installed"
  return 1
}

# Vale exits with an error only for alerts at error level. A warning fails
# the check too.
prose_check() {
  local found
  found=$(vale --minAlertLevel=warning --output=line "${prose[@]}") || true
  [ -z "$found" ] && return 0
  vale --minAlertLevel=warning "${prose[@]}" || true
  return 1
}

run() {
  echo "== $1"
  case "$1" in
    fmt) cargo fmt --check ;;
    clippy) cargo clippy --locked --all-targets --all-features -- -D warnings ;;
    test) cargo test --locked --workspace --all-features ;;
    # The API documentation builds, and every link in it resolves.
    doc) RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features ;;
    vale) if need vale; then prose_check; fi ;;
    typos) if need typos; then typos; fi ;;
    # Licenses, advisories and sources of the crates Hatchery depends on.
    deny) if need cargo-deny; then cargo deny --locked check; fi ;;
    # Links between the Markdown files. Addresses on the web are not fetched.
    links) if need lychee; then lychee --offline --no-progress README.md AGENTS.md docs .github/*.md; fi ;;
    *)
      echo "error: no check named $1" >&2
      exit 2
      ;;
  esac
}

[ $# -gt 0 ] || set -- fmt clippy test doc vale typos deny links
for check in "$@"; do
  run "$check"
done
