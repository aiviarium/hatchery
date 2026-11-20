# From the prototype

The prototype had its own plugin packages and its own installer in the kernel: `.crowplugin` files,
`plugins.lock`, and `crow plugin pack`, `publish`, `add`, `update` and `remove`. Hatchery is that job as a
library and a command of its own, and one package can now serve more than one tool.

| In the prototype | In Hatchery |
| --- | --- |
| `crow plugin pack DIR` writing `<id>-<version>.crowplugin` | `hatchery pack` writing `<id>-<version>.egg` ([Command line](cli.md#pack)) |
| `plugin.json` at the package root, with `id`, `version` and `dependencies` | `package.kdl` at the root, with a target for each tool, and each tool's own manifest in its target's folder ([package.kdl](package-kdl.md)) |
| `plugins.lock` | `eggs.lock`, with `.hatchery/seen.toml` beside it ([eggs.lock](eggs-lock.md)) |
| `crow plugin publish --to REGISTRY --sign-key` | `hatchery publish`, with `hatchery keygen` for the key and `hatchery verify` after it ([Registries](registries.md#signing)) |
| A plugin's own `index.json` | An `index.json` per package, and the registry's catalog at its root ([Registries](registries.md)) |
| `crow plugin add`, `update` and `remove`, which unpacked into `CROW_PLUGIN_DIRS` | `hatchery add`, `install`, `update` and `remove` ([Command line](cli.md#add)), and the client API that a tool calls for its own installs, in [Rust API](api.md) |
| `CROW_PLUGIN_REGISTRIES`, with static folders and URLs | `hatchery.kdl`, with a static registry over HTTPS or in a folder, a git repository, or one folder of a git repository ([Registries](registries.md)) |

## What is new

- One package, several tools. Each target is a folder named for the tool, and a tool reads only its own
  ([Targets](concepts.md#targets)).
- Dependencies and optional dependencies per target, with ranges, and requirements as facts the place of
  installation must have: `os`, `arch`, another tool's version, or a package installed under another tool
  ([Requirements](concepts.md#requirements)).
- Variants for one OS, one architecture or both, and fat eggs that contain them. A tool downloads the base
  egg and only the variant it needs ([Eggs](egg.md)).
- The same input gives the same egg, byte for byte, so a SHA-256 pins a build.
- A minimum release age, which makes a new version wait before it is installed. `hatchery withdraw` takes
  back one that must not be.
- `hatchery pack --check target=schema.json` validates the tool's manifest as it packs, and unpacking
  refuses paths that would escape the install folder.

## Limits

Hatchery treats a target's name as opaque and reads nothing in its folder but what the manifest names. It
does not know what a tool's `plugin.json` means, except through the schema the manifest names.

The prototype's `crow plugin check-interface`, which compared a plugin's `interface.json` against an older
one and refused a breaking change without a major version, is not in this version. Rookery's
[Not in this version](https://rookery.helight.dev/docs/reference/not-in-this-version) lists what the
release leaves out.
