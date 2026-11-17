# package.kdl

`package.kdl` is the manifest of one package. You write it by hand, in KDL version 2. It has three sections: `package` is the identity, `about` is what people see, and `targets` is what gets installed.

```kdl
format 1

package {
  group aiviarium
  id claude
  version "1.4.0"
}

about {
  name "Claude Code"
  summary "Install, sign-in and drivers for Claude Code."
  icon "icon.svg"
  readme "README.md"
  changelog "CHANGELOG.md"
  license MIT
  license Apache-2.0
  author "example"
  homepage "https://example.org/parrot"
  repository "https://github.com/aiviarium/parrot"
  keywords agent claude
}

targets {
  crow {
    build "bun run build:crow"
    requires {
      crow "^1"
    }
    dependencies {
      agents "^1.2"
    }
  }
  rookery manifest="rookery.json" {
    requires {
      rookery ">=0.7"
      os linux
    }
    variants {
      variant arch=x86_64
      variant arch=aarch64
    }
  }
}
```

A folder for that manifest looks like this:

```text
claude/
  package.kdl
  README.md
  icon.svg
  crow/
    plugin.json
    host.js
  rookery/
    rookery.json
    tasks/install.yml
    variants/x86_64/
    variants/aarch64/
```

## format

`format 1` is the first line. Format 1 only gains optional nodes. A change that breaks how a manifest is read gets a new number, which Hatchery reads next to format 1.

## package

| Node | Value | Notes |
| --- | --- | --- |
| `group` | A name | Who publishes the package. Letters, digits, `-`, `_` and `.`, not starting with `.` |
| `id` | A name | The same characters |
| `version` | A complete semver version | One version for the whole package |

`group` and `id` together name the package: `aiviarium/claude`. A `packages.kdl` can supply `group` and `version` for every package of a repository. See [packages.kdl](packages-kdl.md).

## about

Display data. Nothing in `about` affects resolution or installing.

| Node | Value |
| --- | --- |
| `name` | A display name |
| `summary` | One sentence |
| `icon`, `readme`, `changelog` | A path in the package folder. They are packed into the egg. The icon is also copied into a registry for listings |
| `license` | One SPDX identifier per line. The package may be used under one of them |
| `author` | One per line |
| `homepage`, `repository` | URLs |
| `keywords` | Words, on one line or several |

`license`, `author` and `keywords` can be repeated. Values the package leaves out come from `defaults` in `packages.kdl`, and values it sets override them. For `license`, `author` and `keywords`, the defaults apply only when the package has no such line.

## targets

The node name is the target. Names are ASCII letters, digits, `-` and `_`, and never `variants`.

### Properties of a target node

| Property | Meaning |
| --- | --- |
| `path="build/crow"` | The target's folder, when it is not the target's name |
| `manifest="plugin.json"` | The file `hatchery pack --check` validates. Without it, `<target>.json` in the target's folder |

### Children of a target node

| Node | Meaning |
| --- | --- |
| `build "command"` | One command. `hatchery pack` runs it in the package folder before it reads the target's folder |
| `requires { … }` | Facts the place of installation must have. See [Concepts](concepts.md#requirements) |
| `dependencies { … }` | One line per package, with its id and a range |
| `optional-dependencies { … }` | Lines like `dependencies`. Installed when they can be |
| `include`, `exclude` | Glob patterns that narrow which files of the folder are packed. Without them the whole folder is packed |
| `variants { … }` | `variant` lines |

A dependency line is `agents "^1.2"`. A dependency in another group says so on the line: `agents "^1.2" group="other"`. A line with no range means any version.

### variant lines

```kdl
variants {
  variant arch=x86_64
  variant os=linux arch=aarch64
  variant path="out/win" os=windows build="make win"
  variant
}
```

| Property | Meaning |
| --- | --- |
| `os`, `arch` | The keys. Either, both or neither |
| `path` | The folder, relative to the package folder. Without it, `variants/<key>` in the target's folder |
| `build` | A command for this variant. It gets `EGG_OS` and `EGG_ARCH` as well |

A variant with neither key is the fallback, with key `default`. Each key may be listed once.

## Unknown nodes

Hatchery keeps nodes it does not know when it writes a manifest again. A tool can add metadata to a package without a Hatchery release.

## What `pack` writes into the egg

- A complete manifest. Values inherited from the repository's `packages.kdl` are written out, so the egg's `package.kdl` is complete. Target and variant `path=` properties are removed, because inside the egg each target's files are at the default folder.
- An `origin` section, which nobody writes by hand:

```kdl
origin {
  repository "https://github.com/aiviarium/parrot"
  commit "4be1c0d…"
  built "2026-11-10T09:00:00Z"
  hatchery "0.1.0"
}
```

`origin` answers "what source is this egg" after the fact. `built` is `SOURCE_DATE_EPOCH` if set, else the commit time of `HEAD` when the folder is in a git checkout, else it is left out. Packing the same folder twice gives the same egg.
