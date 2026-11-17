# Hatchery documentation

Hatchery packs a package folder into an egg and publishes eggs to a registry. It also resolves versions and installs. One package can hold content for several tools, and each tool installs only its own part.

| Page | What it covers |
| --- | --- |
| [Concepts](concepts.md) | Eggs, targets, requirements, variants, sources, minimum release age and trust |
| [package.kdl](package-kdl.md) | The manifest of one package |
| [packages.kdl](packages-kdl.md) | The root manifest of a repository, with defaults, package locations and releases |
| [hatchery.kdl](hatchery-kdl.md) | The registries an installation searches |
| [eggs.lock](eggs-lock.md) | What is installed and from where |
| [Registries](registries.md) | Static registries, a git repository as a registry, publishing, signing and withdrawing |
| [Command line](cli.md) | Every command and flag, with a CI pipeline |
| [Rust API](api.md) | The client, the three traits, `Egg`, publishing and the errors |
| [Playground](playground.md) | A folder with example packages and a registry to try the command in |
| [Decisions](decisions.md) | What the formats left open and what Hatchery chose |

## Where to start

- To try the command without setting anything up, run the [playground](playground.md).
- To make a package, read [package.kdl](package-kdl.md), then run `hatchery pack`.
- To publish from CI, read the pipeline at the end of [Command line](cli.md) and then [Registries](registries.md).
- To install packages from your own program, read [Rust API](api.md).
