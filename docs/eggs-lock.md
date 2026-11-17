# eggs.lock

`eggs.lock` is in the install folder. Hatchery writes it in TOML. It records what was installed and from where.

```toml
[[package]]
name    = "aiviarium/claude"
version = "1.4.0"
targets = ["rookery"]
git     = "https://github.com/aiviarium/parrot.git"
path    = "agents/claude"
ref     = "v1.4.0"
commit  = "4be1c0d…"
sha256  = "9f2c…"

[package.variants]
"rookery.x86_64" = "51ab…"
```

| Key | Meaning |
| --- | --- |
| `name`, `version` | The package and the installed version |
| `targets` | The targets installed from it |
| `registry` | The address of a static or git registry it came from |
| `git`, `path`, `ref`, `commit` | The repository, the package's folder, the tag or branch, and the commit. For a git registry and for one git folder |
| `file`, `folder` | The egg file or address, or the package folder, it was installed from |
| `sha256` | The base egg's checksum |
| `variants` | The variant eggs fetched so far, as `"<target>.<key>" = sha256` |

Keys that do not apply are left out. Packages are sorted by name.

## Using the lock

- `hatchery install` installs exactly what the lock lists, and checks checksums and commits.
- If a tag later points at a different commit, Hatchery refuses to install and says so.
- Installs from the lock are not held by the minimum release age.
- `hatchery update` resolves again and rewrites the entries it changes.

## The install folder

```text
<dir>/eggs.lock
<dir>/<group>/<id>/                          the unpacked base egg
<dir>/<group>/<id>/<target>/variants/<key>/  each variant that was fetched
<dir>/.hatchery/seen.toml                    when each version was first seen in each registry
```

`seen.toml` backs the minimum release age. Keep it with the install folder.

To compare an installed package with a copy elsewhere, `hatchery::tree_digest` gives a checksum of a folder's files. It covers the paths, the executable bit of each file and the contents.
