# eggs.lock

`eggs.lock` is in the install folder. Hatchery writes it in TOML. It records what was installed and from where.

```toml
format = 1

[[package]]
name     = "aiviarium/claude"
version  = "1.4.0"
targets  = ["rookery"]
registry = "https://github.com/aiviarium/parrot.git"
path     = "agents/claude"
ref      = "v1.4.0"
commit   = "4be1c0d…"
sha256   = "9f2c…"

[package.variants]
"rookery.x86_64" = "51ab…"
```

| Key | Meaning |
| --- | --- |
| `format` | The format of the file, 1. A file without it is format 1. Another number is refused |
| `name`, `version` | The package and the installed version |
| `targets` | The targets installed from it |
| `registry` | The address of a static or git registry it came from |
| `git` | The repository of one git folder, installed with `--git`. A git registry sets `registry` instead |
| `path`, `ref`, `commit` | The package's folder, the tag or branch, and the commit. For a git registry and for one git folder |
| `file`, `folder` | The egg file or address, or the package folder, it was installed from |
| `sha256` | The base egg's checksum |
| `variants` | The variant eggs fetched so far, as `"<target>.<key>" = sha256` |

Keys that do not apply are left out, and packages are sorted by name. When Hatchery writes the file again, it keeps the keys it does not know, at the top and in each package.

## Using the lock

- `hatchery install` installs exactly what the lock lists, and checks checksums and commits. It skips a package only when its folder contains the locked version, unpacked from the egg with the locked checksum. So `install` applies a lock that changed, such as after a pull from version control.
- If a tag later points at a different commit, Hatchery refuses to install and says so.
- Installs from the lock are not held by the minimum release age.
- `hatchery update` resolves again and rewrites the entries it changes.

## The install folder

```text
<dir>/eggs.lock
<dir>/<group>/<id>/                          the unpacked base egg
<dir>/<group>/<id>/<target>/variants/<key>/  each variant that was fetched
<dir>/.hatchery/seen.toml                    when each version was first seen in each registry
<dir>/.hatchery/unpacked.toml                the checksum of the base egg each package folder was unpacked from
```

An install folder refuses a package whose group, or whose id within one group, differs from another only by case, because Windows and macOS would put both in one folder.

`seen.toml` backs the minimum release age. Keep it with the install folder. It and `unpacked.toml` have `format = 1` too, and keep keys they do not know. `unpacked.toml` tells `install` what each folder contains. Without it, `install` unpacks every package again.

`hatchery list` shows the version in each package's own `package.kdl`, which is what is installed. It can differ from the lock until `install` runs.

To compare an installed package with a copy elsewhere, `hatchery::tree_digest` gives a checksum of a folder's files. It covers the paths, the executable bit of each file and the contents.
