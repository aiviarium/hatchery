# Working on Hatchery

Hatchery is a Rust library and a command. It turns package folders into eggs and publishes them. It also resolves versions to install. This file is for coding agents working in this repository.

## Layout

| Path | What it is |
| --- | --- |
| `crates/hatchery/` | The library. `lib.rs` lists the modules and the public names |
| `crates/hatchery/src/manifest.rs`, `repo.rs`, `config.rs` | The parsers and writers of `package.kdl`, `packages.kdl` and `hatchery.kdl`. `kdlx.rs` holds the KDL helpers |
| `crates/hatchery/src/egg.rs`, `archive.rs`, `pack.rs` | Eggs, the tar and gzip writer and reader, and `hatchery pack` |
| `crates/hatchery/src/publish.rs`, `index.rs`, `sign.rs` | Publishing, the JSON of a static registry and Ed25519 signatures |
| `crates/hatchery/src/source/` | The sources, which are static and git registries, one git folder, a folder and a file |
| `crates/hatchery/src/client.rs`, `resolve.rs`, `lock.rs`, `seen.rs` | The client, version resolution, `eggs.lock` and the record of when versions were first seen |
| `crates/hatchery/src/git.rs`, `fetch.rs`, `fetch_http.rs`, `exec.rs` | The `git` command and its cache, the `Fetch` trait with its ureq version, and `block_on` |
| `crates/hatchery/tests/` | Integration tests. `tests/common/mod.rs` has package folders, git helpers and a fixed clock |
| `crates/hatchery-cli/` | The `hatchery` command. `args.rs` is the grammar in clap, and one module for each group of commands |
| `docs/` | The documentation, in Markdown |
| `examples/` | Example packages and the playground scripts. See `docs/playground.md` |
| `ci/checks.sh` | The checks CI runs |
| `.vale/` | The prose rules |

## Build and check

CI runs one script, and so can you:

```bash
ci/checks.sh
```

Name checks to run only those, as in `ci/checks.sh fmt clippy`. The table lists the names. A check whose tool you have not installed is skipped on your machine and fails in CI.

| Check | Tool | What it covers |
| --- | --- | --- |
| `fmt`, `clippy`, `test` | cargo | Formatting, lints as errors, and the tests |
| `doc` | cargo | The API documentation builds with no warning, and a public item without a doc comment is one |
| `vale` | [Vale](https://vale.sh) | The prose rules in `.vale/` over the Markdown files |
| `typos` | [typos](https://github.com/crate-ci/typos) | Spelling in code and docs. Exceptions go in `_typos.toml` |
| `deny` | [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) | Licenses, advisories and sources of dependencies, by `deny.toml` |
| `links` | [lychee](https://lychee.cli.rs) | Links between the Markdown files |

CI also builds with the Rust version in `rust-version`, and runs `examples/playground.sh` and `examples/playground-check.sh`.

The tests use `git`, which has to be installed. They run it with a fixed identity and no user or system configuration.

## Conventions

- Commits follow Conventional Commits (`feat(pack): …`, `fix(publish): …`, `docs: …`, `ci: …`). Write a short subject, and add a body only when it explains something the diff does not.
- A comment gives the reason for the code in a sentence. Neither code nor docs refer to plans or decisions made elsewhere.
- Errors are variants of `Error` in `error.rs`, with the package, version and registry in an `At`. Use `Error::invalid` for bad data and `IoContext::at_path` for I/O. Never panic on input.
- Futures are `BoxFuture` from `fetch.rs`. There is no async runtime in the library. Tests drive futures with `hatchery::block_on`.
- Eggs are written deterministically. A change to the writers keeps the same input giving the same SHA-256.
- A change to a file format keeps format 1 additive: only optional nodes. Unknown nodes are kept when a file is written again.
- Documentation is written for people, in short sentences and with tables for lookups. It has to pass vale.
- Every public item of the library has a doc comment that says what a caller needs to know.
- A change to what a command prints or accepts updates `docs/cli.md`, and `docs/playground.md` and `examples/playground-check.sh` when a step there changes.
- Tests and examples use made-up names such as `example` and `demo`, and `example.org` addresses. The names `crow` and `rookery` are fine as made-up target names.
- Secrets never go into argv, logs or the repository. The signing key and tokens are read from environment variables.
- Crate dependencies stay few. Add one only when the task needs it.

## Versions and releases

- The version is `version` in `[workspace.package]` of `Cargo.toml`. `hatchery --version` and the `origin` of an egg say it.
- A release is a pushed tag `vX.Y.Z` that matches that version. The Release workflow builds `hatchery` for each system and packs it with the licenses and a `.sha256`. It then publishes the release.
- A tag with a suffix, such as `v0.2.0-rc.1`, makes a pre-release.
