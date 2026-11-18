<h1><a href="#license"><img align="right" src="https://aiviarium-badges.helight.workers.dev/hatchery/github/license/aiviarium/hatchery" alt="License"></a><a href="https://github.com/aiviarium/hatchery/actions/workflows/ci.yml"><img align="right" src="https://aiviarium-badges.helight.workers.dev/hatchery/github/actions/workflow/status/aiviarium/hatchery/ci.yml?branch=main&label=CI" alt="CI"></a><a href="https://github.com/aiviarium/hatchery"><picture><source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/aiviarium/.github/main/brand/lockups/lockup-hatchery.svg"><img src="https://raw.githubusercontent.com/aiviarium/.github/main/brand/lockups/lockup-hatchery-light.svg" alt="Hatchery" height="56"></picture></a></h1>

Packaging and distribution for plugins and modular software.

Hatchery is a Rust library and a command, `hatchery`. Use it to pack a package folder into an egg and publish the egg to a registry. It also resolves versions, verifies eggs and installs them.

A package can hold content for several tools. Each tool, called a target, installs only its own part. An integration for two tools can then be one versioned package.

Hatchery is not a plugin runtime. It doesn't read what a package contributes or run a package's code. Each tool that installs packages has its own manifests, loaders and execution.

Hatchery is early, and the file formats and the API can still change before 1.0.

## Install

Download the program for your system from the [releases](https://github.com/aiviarium/hatchery/releases). Each archive has a `.sha256` file. They hold `hatchery` and the licenses, for Linux (x86_64 and aarch64, static), macOS and Windows.

To build it, you need Rust 1.95 or newer:

```bash
cargo install --locked --git https://github.com/aiviarium/hatchery hatchery-cli
```

Installing packages with git registries needs the `git` command, version 2.25 or later.

## Quick start

A package is a folder with a `package.kdl`:

```kdl
format 1

package {
  group example
  id hello
  version "0.1.0"
}

about {
  name "Hello"
  summary "A first package."
  license MIT
}

targets {
  crow {
    requires {
      crow "^1"
    }
  }
}
```

Put the files for the target in `crow/`. Then pack, look and publish:

```bash
hatchery pack hello --out dist
hatchery inspect dist/hello-0.1.0.egg
hatchery publish dist/hello-0.1.0.egg --to ./registry
```

On the machine that installs it, list the registry once and add the package for your tool:

```bash
hatchery registry add dir ./registry
hatchery add example/hello --target crow --fact crow=1.2.0 --dir ./plugins
hatchery list --dir ./plugins
```

## Use it from Rust

```rust
use hatchery::fetch::HttpFetch;
use hatchery::{Client, Config, Spec, StaticTarget, block_on};

let client = Client::builder()
    .config(Config::load(std::path::Path::new("hatchery.kdl"))?)
    .target(StaticTarget::new("crow").fact("crow", "1.2.0"))
    .fetch(HttpFetch::new())
    .build()?;
let report = block_on(client.add(&Spec::parse("example/hello@^0.1")?, "./plugins".as_ref()))?;
```

`HttpFetch` needs the `http` feature. A tool writes a `Target` to answer the requirements of its own packages.

## Documentation

- [Concepts](docs/concepts.md) covers eggs, targets, requirements, variants, sources, minimum release age and trust.
- [package.kdl](docs/package-kdl.md), [packages.kdl](docs/packages-kdl.md), [hatchery.kdl](docs/hatchery-kdl.md) and [eggs.lock](docs/eggs-lock.md) are the file formats.
- [Registries](docs/registries.md) covers static registries, a git repository as a registry, publishing, signing and withdrawing.
- [Command line](docs/cli.md) lists every command and flag, and shows a CI pipeline.
- [Rust API](docs/api.md) describes the library.
- [Playground](docs/playground.md) sets up example packages and a registry to try the command in.
- [Decisions](docs/decisions.md) lists what the formats left open and what Hatchery chose.

## Contributing and security

[CONTRIBUTING.md](.github/CONTRIBUTING.md) says what Hatchery is meant to be and how changes get in. Open an issue before a pull request. [SECURITY.md](.github/SECURITY.md) says how to report a vulnerability.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
