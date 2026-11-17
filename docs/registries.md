# Registries

A registry is where eggs are published and found. Hatchery has two layouts. A static registry is a folder of files. A git repository with a `packages.kdl` is the other. You list either in [hatchery.kdl](hatchery-kdl.md).

## Static registry

A static registry is a folder of files, served from a directory or over HTTP. GitHub Pages, release assets and a raw Sonatype repository all work.

```text
<base>/index.json
<base>/aiviarium/claude/index.json
<base>/aiviarium/claude/icon.svg
<base>/aiviarium/claude/claude-1.4.0.egg
<base>/aiviarium/claude/claude-1.4.0.rookery.x86_64.egg
<base>/aiviarium/claude/claude-1.4.0.rookery.aarch64.egg
```

`<base>/index.json` lists the packages:

```json
{ "packages": ["aiviarium/claude"] }
```

`<base>/<group>/<id>/index.json` is the package index. The index is JSON because `hatchery publish` writes it and scripts read it.

```json
{
  "group": "aiviarium",
  "id": "claude",
  "about": {
    "name": "Claude Code",
    "summary": "Install, sign-in and drivers for Claude Code.",
    "icon": "icon.svg",
    "licenses": ["MIT", "Apache-2.0"],
    "authors": ["example"],
    "homepage": "https://example.org/parrot",
    "repository": "https://github.com/aiviarium/parrot",
    "keywords": ["agent"]
  },
  "versions": {
    "1.4.0": {
      "sha256": "9f2c…",
      "signature": "…",
      "published": "2026-11-10T09:12:00Z",
      "withdrawn": null,
      "targets": {
        "crow": {
          "build": true,
          "requires": { "crow": "^1" },
          "dependencies": { "aiviarium/agents": "^1.2" },
          "optional-dependencies": {}
        },
        "rookery": {
          "requires": { "rookery": ">=0.7", "os": "linux", "gpu": true },
          "variants": [
            { "arch": "x86_64", "sha256": "51ab…", "signature": "…", "published": "…" },
            { "arch": "aarch64" }
          ]
        }
      }
    }
  }
}
```

| Field | Meaning |
| --- | --- |
| `sha256`, `signature`, `published` | Of the base egg. The signature is lowercase hex |
| `withdrawn` | `null`, or `{"reason": "…"}` with the reason optional |
| `targets.<name>.requires` | One value as a string, several as an array, none as `true` |
| `targets.<name>.variants` | Each variant with its own `sha256`, `signature` and `published` once its egg is published |
| `about.icon` | The file name of the icon copy beside the index |

The index repeats the display data and each target's requirements, dependencies and variant keys. A listing can show a package and say why it cannot be installed.

Empty maps, `build: false` and null signatures may be left out when writing. Reading accepts both. Unknown keys at any level are kept when the file is written again.

A version entry without `sha256` has only variant eggs published so far. It is not offered, because a version becomes visible when its base egg is published.

### Reading a static registry

- Each variant egg has its own checksum and signature in the index, so it can be published and verified on its own.
- A registry with a `key` refuses an egg without a valid signature (`SignatureInvalid`).
- A checksum that differs from the index is `ChecksumMismatch`.
- The source also checks that the egg's manifest has the offered package and version.

## Git repository as a registry

A repository with a `packages.kdl` is listed once, and its packages are then installed by name.

```kdl
registries {
  git "https://github.com/aiviarium/parrot.git" only="aiviarium"
}
```

- What it offers is read from `packages.kdl` on the default branch.
- Versions are tags, matched against the tag pattern. The version in the manifest at the tag must match the tag.
- Ranges and update checks read the tag list with `git ls-remote`. 
- Fetching is a shallow, partial clone. Hatchery reads the package's folder from it. The files are the committed bytes, with no line-ending conversion or filters from `.gitattributes` or your git configuration. One cached copy per repository is reused for all its packages.
- The lock file records the commit. If a tag later points at a different commit, Hatchery refuses to install and says so.
- Access is git's own, through SSH keys, credential helpers or a token in CI. A private repository takes no Hatchery setting.
- For development, a branch works as a ref (`--ref main`), and an update resolves it again.
- The `git` command, version 2.25 or later, must be installed where eggs are installed.

### Targets that need a build

A tag holds source code, and Hatchery never builds at install. A target with no build command is installed straight from the tag. For every other target, CI publishes the built egg to the place `release.eggs` names.

- For each version Hatchery looks in the eggs branch first.
- It falls back to the tag's source only when there is no egg for the version and none of the wanted targets has a build.
- Otherwise it fails with `NotBuilt`.
- Whoever installs lists the repository once. Their side stays the same when a package gains its first built target.

The eggs branch has the static registry layout, with the checksum, signature and publish time of each egg. It grows with every binary you publish. If that becomes a burden, the same files can move to a static registry without changing the eggs.

## Publishing

```bash
hatchery publish dist/claude-1.4.0.egg
```

With no destination, `publish` finds `packages.kdl` from the current folder upward, reads `release.eggs` and publishes to the `origin` remote of that checkout. `--to DIR` publishes to a static registry folder. `--git URL` publishes to a branch of another repository. See [Command line](cli.md#publish).

A version becomes visible when its base egg is published. CI publishes the variant eggs first, one from each build job, and the base egg last. `hatchery publish` also sorts variant eggs before base eggs within one call. A variant can be added to a released version later.

An egg published again with the same bytes is a no-op. With different bytes it is an error, because versions are immutable.

A package with no built target is not published. The tag is the release.

## Signing

Eggs are signed with Ed25519. The signature covers the package, version, file name, checksum and publish time. Because it covers `published`, the publish time cannot be backdated without the key.

1. Make a key pair with `hatchery keygen`.
2. Store the secret key in your CI as a secret, and expose it as an environment variable.
3. Publish with `--sign-key-env VARIABLE`.
4. Give readers the public key as `key="ed25519:…"` in their `hatchery.kdl`.

The signing key never goes on a command line. Hatchery reads it from the variable you name.

To check an egg against a registry by hand:

```bash
hatchery verify dist/claude-1.4.0.egg --registry ./registry --key ed25519:edaea96ff0df28e04262ffeb14297bf3131d1daee5938d0ec2d47a9c6af363cd
```

## Withdrawing a version

Withdrawing marks a published version as not to be installed. Hatchery skips it when resolving and warns where it is already installed.

| Registry | How |
| --- | --- |
| A static registry folder | `hatchery withdraw aiviarium/claude@1.3.2 --to ./registry --reason "…"` |
| A git repository | Add a `withdrawn` line to `release` in `packages.kdl` and push it |

The line in `packages.kdl` looks like this:

```kdl
release {
  withdrawn claude "1.3.2" reason="broke sign-in on Ubuntu 26.04"
}
```
