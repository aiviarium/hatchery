# Contributing to Hatchery

Thanks for wanting to help. This page says what Hatchery is trying to be, and
how a change gets from an idea to `main`. Coding agents working in the
repository should also read [AGENTS.md](../AGENTS.md).

## What Hatchery is

Hatchery turns a package folder into an egg and publishes eggs to a registry. It resolves versions and installs. A package can hold content for several tools. Each tool, called a target, installs only its own part.

It is kept small on purpose:

- **Not a plugin runtime.** Hatchery doesn't read what a plugin contributes or run plugin code. Each tool that installs packages has its own manifests, loaders and execution.
- **Targets are opaque.** A new tool is a new name under `targets` and a `Target` in that tool's repository. This crate has no tool's name in it.
- **No build or scripts at install.** `hatchery pack` runs the build command the package's author wrote. Installing runs nothing.
- **Few formats.** People write KDL, the registry index is JSON and the lock file is TOML. Git work goes through the `git` command. The crate has few dependencies.
- **Additive formats.** Format 1 only gains optional nodes. A change that breaks
  how an existing package is read is a new format number.

A change that makes Hatchery do more is weighed against these points. Doing the
same with less is always welcome.

## Before you write code

**Open an issue first.** This goes for features, for changes in behaviour and
for anything larger than an obvious fix. Say what you want to do and why. The
scope should be small enough to review in one sitting.

| Form | For |
| --- | --- |
| Feature | Something Hatchery can't do at all yet |
| Improvement | Something Hatchery does already, but could do more easily, faster or more clearly |

Once the issue is there, you can begin on the pull request straight away. A
feature pull request with no issue behind it will be closed, and so will a
first contribution from an account with no earlier issues.

Typo fixes, broken links and small documentation corrections can go straight
to a pull request.

## Using language models

Using a model to help is fine. What we ask:

- **You review it.** Read every line of a change before you submit it, and be
  able to explain it.
- **No unattended submissions.** Issues and pull requests opened by an agent
  with nobody reading them will be closed.
- **Prose passes Vale.** The repository has style rules that catch the usual
  machine phrasing in documentation.

## Making the change

You need a Rust toolchain at the version in `rust-version` and `git`. CI runs
these:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
vale --minAlertLevel=warning README.md AGENTS.md CLAUDE.md docs .github/*.md
```

A pull request is ready when:

- The checks above pass.
- Behaviour that changed has a test. A change to a file format has a fixture.
- Documentation under `docs/` is updated in the same pull request, and Vale
  doesn't report warnings.
- Commits follow [Conventional Commits](https://www.conventionalcommits.org/),
  with a short subject that says what changed.
- The pull request links its issue and stays within it.

## Reporting bugs

Open an issue with what you ran, what you expected and what happened. The
output of `hatchery --version` helps. Take addresses, tokens and keys out
before you paste anything.

Don't report security problems in an issue. [SECURITY.md](SECURITY.md) says
where they go.

## Licence

Hatchery is licensed under either of the [Apache License, Version 2.0](../LICENSE-APACHE)
or the [MIT license](../LICENSE-MIT), at your option. By contributing you agree
that your contribution is licensed the same way, with no further terms.
