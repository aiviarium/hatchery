# packages.kdl

`packages.kdl` sits at the root of a repository that holds packages. It gives the values shared by all packages and says where the packages are. It also says how releases are tagged and where built eggs go. It is also what lets the repository serve as a registry.

```kdl
format 1

defaults {
  package {
    group aiviarium
    version "1.4.0"
  }
  about {
    license MIT
    license Apache-2.0
    author "example"
    repository "https://github.com/aiviarium/parrot"
  }
}

packages {
  find "agents/*"
  package "shared/agent-base" tag="agent-base-v{version}"
}

release {
  tag "v{version}"
  eggs branch="eggs"
  withdrawn claude "1.3.2" reason="broke sign-in on Ubuntu 26.04"
}
```

## defaults

`defaults` has the same `package` and `about` sections as a package manifest. A `package.kdl` that leaves a value out takes it from here. A value it sets overrides the default.

With the version here and one tag per release, a release is a one-line change. A package manifest can then shrink to its identity and its targets:

```kdl
format 1

package {
  id codex
}

about {
  name "Codex"
  summary "Install, sign-in and drivers for Codex."
}

targets {
  rookery {
    requires {
      rookery ">=0.7"
    }
  }
}
```

## packages

| Line | Meaning |
| --- | --- |
| `find "agents/*"` | Every folder matching the glob that has a `package.kdl` |
| `package "shared/agent-base"` | One folder |

A package's id is read from its own manifest, and `defaults` cannot give one. Either line can set its own tag pattern with `tag=`. Folders named by a `package` line come first, then those found, each once.

## release

| Line | Meaning |
| --- | --- |
| `tag "v{version}"` | The tag pattern for all packages. It holds `{version}` once, and may hold `{id}`. Without it, `v{version}` |
| `eggs branch="eggs"` | The branch that holds built eggs, `eggs` when `branch` is left out. `path="registry"` places them in a folder of that branch |
| `withdrawn ID "VERSION" reason="…"` | Marks a released version as not to be installed. The package is an id or `group/id`. `reason` is optional |

`hatchery publish` with no destination reads `release.eggs` and publishes to the `origin` remote of the checkout. See [Registries](registries.md).

A tag cannot be taken back, so `withdrawn` is how a bad release is pulled. Hatchery skips a withdrawn version when resolving and warns where it is already installed.

## Finding the file

`hatchery pack` and `hatchery publish` look for `packages.kdl` in the package folder and then in each folder above it. The search stops at the root of a git checkout.

## Unknown nodes

Nodes Hatchery does not know are kept.
