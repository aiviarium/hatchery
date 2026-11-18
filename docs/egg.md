# Eggs

An egg is a built package, stored as a gzipped POSIX ustar archive. `hatchery pack` writes eggs, and `hatchery inspect` shows what is in one. This page is the layout, for anyone who reads or writes eggs without Hatchery.

## The archive

Hatchery writes the same bytes for the same input, so the SHA-256 of an egg depends only on its files.

- Entries are sorted by path.
- Entries are files only. There are no directory entries.
- The mode is 0644, or 0755 for an executable file.
- User and group ids are 0, the names are empty, and the modification time is 0.
- The gzip header is fixed.

## Base egg

A base egg is named `<id>-<version>.egg`, such as `claude-1.4.0.egg`. It holds:

| Path | What it is |
| --- | --- |
| `package.kdl` | The complete manifest, with an `origin` section. See below |
| The files `about` names | The icon, readme and changelog, at their paths in the package folder |
| `<target>/<path>` | The shared files of each target |

The `package.kdl` in an egg is complete. Values the package took from `defaults` in [packages.kdl](packages-kdl.md) are written out. Every target and variant `path=` is removed, because inside the egg each target's files are in the folder of its name.

`pack` adds an `origin` section:

```kdl
origin {
  repository "https://github.com/aiviarium/parrot"
  commit "4be1c0d…"
  built "2026-11-10T09:00:00Z"
  hatchery "0.1.0"
}
```

| Node | Value |
| --- | --- |
| `repository` | `about.repository`, else the `origin` remote of the checkout, without user and password |
| `commit` | The commit of `HEAD`, when the folder is in a git checkout |
| `built` | `SOURCE_DATE_EPOCH` if set, else the commit time of `HEAD`. Left out when there is neither |
| `hatchery` | The version of Hatchery that packed it |

Because `built` doesn't come from the clock, packing the same folder twice gives the same egg.

## Variant egg

A variant egg is named `<id>-<version>.<target>.<key>.egg`, such as `claude-1.4.0.rookery.x86_64.egg`. The key is the variant's values joined by a dash, `os` first, or `default` for the fallback. It holds:

| Path | What it is |
| --- | --- |
| `variant.kdl` | The header |
| `<target>/variants/<key>/<path>` | The variant's files |

The header gives the package and the variant:

```kdl
format 1
variant {
  group aiviarium
  id claude
  version "1.4.0"
  target rookery
  arch x86_64
}
```

`os` and `arch` are there when the variant has them. The fallback variant has neither.

## Fat egg

A fat egg is named like the base egg. It holds the entries of the base egg and, for each variant, `variants/<variant egg file name>` with the complete bytes of that variant egg. A fat egg installs without a registry.

Removing the `variants/` entries and writing the archive again gives the base egg, byte for byte. The base checksum of a fat egg is the checksum of that base egg.

## Names an egg may not hold

Unpacking refuses an egg with any of these, and `hatchery pack` refuses to pack them:

- `..`, an absolute path, a symlink or a hardlink.
- A `.git` component in any case, or `git~1`. A `.git` folder in an install folder would run code when someone runs git there.
- A component that ends in a dot or a space, holds a character Windows does not allow, such as `:`, or is a Windows device name such as `con` or `nul.txt`.
- Paths that differ only by case, such as `README` and `readme`.
