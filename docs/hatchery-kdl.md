# hatchery.kdl

`hatchery.kdl` says which registries an installation searches and how long new versions wait. A tool such as an app that embeds Hatchery keeps one in its own configuration folder and passes its path. The `hatchery` command finds one by itself.

```kdl
default min-age="3d"

registries {
  git "https://github.com/aiviarium/parrot.git" only="aiviarium" min-age="0d"
  http "https://eggs.example.org" key="ed25519:edaea96ff0df28e04262ffeb14297bf3131d1daee5938d0ec2d47a9c6af363cd" min-age="7d" token-env="EGGS_EXAMPLE_TOKEN"
  dir "/srv/eggs"
}
```

## Finding the file

In order:

1. `--config PATH`.
2. The file named by the environment variable `HATCHERY_CONFIG`.
3. `hatchery.kdl` in the current folder.
4. `hatchery/hatchery.kdl` in the user's configuration folder: `$XDG_CONFIG_HOME`, else `~/.config`, and `%APPDATA%` on Windows.

With none of them, the configuration is empty.

## Registry lines

The node name is the kind of registry, and its argument is the location.

| Kind | Location | Reads |
| --- | --- | --- |
| `git` | A repository address or a local folder | `packages.kdl` and tags, and the `release.eggs` branch for built eggs |
| `http` | A base URL | A [static registry](registries.md) over HTTP |
| `dir` | A folder | A static registry on disk |

A tool can add a kind with `ClientBuilder::kind`, backed by its own `Source`.

| Property | Meaning |
| --- | --- |
| `only="a,b"` | Serve only these groups. Without it, a registry can serve any group. This stops a second registry from answering for a group that is yours |
| `key="ed25519:…"` | The registry's public key. Eggs from it must be signed. For a `git` registry, a version that has only a tag is not installed |
| `unsigned-tags=#true` | For a `git` registry with a `key`: install versions that have only a tag from source, unsigned. Anyone who can push a tag can then release. Off by default |
| `min-age="3d"` | The minimum release age for this registry |
| `token-env="VAR"` | The environment variable with a bearer token for it. It must be set. Hatchery sends the token over `https://`, and over plain `http://` only to this machine |

Other properties are kept and given to a tool's own `kind`.

The order of the lines is the search order. The first registry that has a package serves it.

## default

`default min-age="3d"` sets the age for registries without their own. Without it, a registry read over the network is held for 3 days and a local folder is not held. A `git` location that is a folder, and any `dir`, count as local.

A duration is a number and a unit: `s`, `m`, `h`, `d` or `w`. `0` is also accepted.

## Secrets

The file has no secrets in it. `token-env` names a variable, and signing keys are passed the same way. Hatchery reads environment variables for secret values and for `HATCHERY_CONFIG`, and for no other setting.

## Editing

`hatchery registry add`, `registry remove` and the `Config` type in the library edit the file in place. Comments and layout are kept.
