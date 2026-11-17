# Concepts

## Eggs

An egg is a built package. It is a gzipped tar named `<id>-<version>.egg` with the manifest, the files the manifest points at and each target's files.

Hatchery writes eggs the same way every time. Entries are sorted, timestamps and owners are fixed, and the gzip header is fixed. The same input gives the same SHA-256.

Eggs come in three kinds:

| Kind | Name | Holds |
| --- | --- | --- |
| Base egg | `claude-1.4.0.egg` | `package.kdl`, the files `about` names, and each target's shared files |
| Variant egg | `claude-1.4.0.rookery.x86_64.egg` | A header, `variant.kdl`, and the files of one variant |
| Fat egg | `claude-1.4.0.egg` | A base egg with variant eggs inside and installs from one file |

Hatchery reads all three everywhere. Removing the variant eggs from a fat egg gives the base egg back, byte for byte.

Unpacking refuses `..`, absolute paths, symlinks and hardlinks. It also refuses a `.git` folder, and the names Windows treats as `.git`. It refuses names Windows cannot hold, such as `con`, `nul.txt`, `a:b` or a name that ends in a dot or a space. It refuses two paths that differ only by case. `hatchery pack` refuses the same names.

## Targets

A target is a tool that installs packages, such as `crow` or `rookery`. It is also the name of the content a package has for that tool.

Hatchery treats the name as opaque. A tool reads its own target and never sees the others. A package folder keeps each target's files in a folder of the same name, so `crow` reads `crow/`.

Hatchery reads nothing in a target's folder except what the manifest names. The target's own manifest, such as `plugin.json`, belongs to the tool.

## Requirements

A target can say what it needs of the place it is installed. Each line in its `requires` block is a fact name and the values it accepts:

```kdl
requires {
  rookery ">=0.7"
  os linux macos
  gpu
}
```

Every line must hold, or the target is not installed. The error gives the fact. There is no `and`, `or` or `not`.

Hatchery knows two facts, `os` and `arch`. The tool that installs the target supplies the rest. A match follows these rules:

1. A name with no value means the fact must be present.
2. A version fact is compared with a range value as versions.
3. Otherwise the fact must equal one of the listed values.

| Fact | Who answers | Example |
| --- | --- | --- |
| The tool's own name | The tool, with its version | `crow "^1"` |
| Another tool's name | A tool that is connected to it. Absent otherwise | `rookery ">=0.7"` under `crow` |
| A package in another tool | The tool, from what is installed there | `"rookery:aiviarium/agent-base" "^1"` |
| `os`, `arch` | Built in. A tool can override them | `os linux macos` |
| Anything else | The tool | `gpu` |

## Dependencies

Dependencies are per target. A dependency without a group is in the package's own group. `optional-dependencies` works the same way. Those packages are installed when they can be and skipped when they cannot.

Ranges use Cargo's syntax. `"1.2"` means `^1.2`.

## Variants

A variant is a folder laid over a target's own files for one OS, one architecture, or both. Those are the only two keys.

- A variant's folder is `variants/<key>` in the target's folder. The key is its values joined by a dash: `x86_64`, `linux`, `linux-x86_64`.
- The first variant listed that matches is used.
- A variant with neither key is the fallback. Its key is `default`. It is used only when no keyed variant matches, wherever it is listed.
- If variants are declared and none matches, the install fails with `NoVariant`.
- Each variant is its own egg. A tool that installs for this machine downloads the base egg and the one matching variant. Other platforms' variants are not downloaded.
- A tool that installs for other machines can fetch a variant when it first needs it, with `Client::variant`.

The `variants` folder is never part of the shared files.

## Build

A target can have a `build` command. `hatchery pack` runs it in the package folder before it reads the target's folder, with `EGG_ID`, `EGG_VERSION` and `EGG_TARGET` set. A variant's own `build` also gets `EGG_OS` and `EGG_ARCH`.

Without a variant build, the variant's folder must exist already, such as from a CI matrix. `hatchery pack --no-build` packs what is there.

Installing builds nothing and runs none of a package's code.

## Sources

| Source | How it is named | Use |
| --- | --- | --- |
| Git repository | `aiviarium/claude@^1.4`, with the repository listed as a registry | Packages come from `packages.kdl`, versions from tags. Resolves ranges and offers updates |
| Static registry | `aiviarium/claude@^1.4`, with a folder or URL listed as a registry | The same, from a folder of files |
| One git folder | `--git URL --path agents/claude --ref v1.4.0` | One folder at one tag, branch or commit. No ranges |
| Folder | `./claude` | Development |
| File | `./claude-1.4.0.egg` or an `https://` address | Uploads, air-gapped installs, release assets. A fat egg installs without a registry |

A package can be named by its id alone when only one group in the configured registries has it. If two groups have the same id, the error is `Ambiguous` and you name the group.

A spec such as `aiviarium/claude@1.4.0` with a complete version means exactly that version. Any other text after `@` is a range.

## Minimum release age

A new version is not installed until it has been public for a set time. Most malicious releases are found and pulled within days.

- The setting is per registry, with a `default` line for the rest. Without either, a registry read over the network is held for 3 days. A local folder is not held.
- For built eggs, `hatchery publish` writes `published` into the index, and the signature covers it.
- A tag's date is set by its author. For tags, the age counts from when this installation first saw the tag.
- Hatchery remembers which versions it has seen in each registry. A version that shows up later with an old publish time counts from when it was first seen.
- The resolver skips versions that are too young, for the package you asked for and for every dependency. If an older version fits the range, it installs that one. If none does, it fails with `TooYoung` and the time the version becomes installable.
- `--allow-young` overrides the hold.
- Installs from `eggs.lock` are not held, since the checksum or commit pins them. Folders and files are not held either.

## Trust

| Source | What protects it |
| --- | --- |
| A built egg in a registry with a `key` | The Ed25519 signature over the package, version, file name, checksum and publish time |
| A built egg in a registry without a key | The checksum in the index. The index is only as safe as the transport |
| A tag's source in a git registry | Git's transport and the commit in `eggs.lock`. A tag that later points elsewhere is refused with `TagMoved` |
| A folder or a file | You chose the exact one |

A registry with a key refuses an unsigned egg. The signing key is never in `hatchery.kdl` or on a command line. It travels in an environment variable.

## What Hatchery leaves out

- No task graph, cache or incremental build. The build command is a script you own.
- No scripts at install.
- No package-level features and no condition expressions.
- No conflicts or replaces.
- No permissions list. A target declares what it may do in the tool's own manifest.
- No translated display text. `about` is in one language.
