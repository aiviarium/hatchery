# Playground

The playground is a folder to try `hatchery` in. It holds a git repository of example packages with two releases, and a configuration that lists that repository as a registry. Everything stays inside the folder, including the cache and the git settings.

```bash
examples/playground.sh
. target/playground/env.sh
```

The script builds `hatchery` and writes the folder. Sourcing `env.sh` puts that `hatchery` on your `PATH`, points it at the playground's configuration and moves you into the folder. Run the script again to start over.

## What is in it

| Path | What it is |
| --- | --- |
| `aviary.git` | The repository, as a hosting service would hold it. It has the tags `v1.0.0` and `v1.1.0`, and an `eggs` branch with built, signed eggs |
| `aviary/` | A checkout of it, where you edit and release packages |
| `hatchery.kdl` | The configuration, with `aviary.git` as its one registry, its public key and `unsigned-tags=#true` |
| `sign.key` | The signing key of the playground. `env.sh` exports it as `HATCHERY_SIGN_KEY` |

The repository has two packages, both in the group `example`:

| Package | Targets | Shows |
| --- | --- | --- |
| `hello` | `crow` | A package with no build. Its tag is its release |
| `toolbox` | `crow`, `rookery` | A dependency on `hello`, a build command, requirements, and variants for two architectures |

The sources are in [examples/aviary](../examples/aviary).

The registry line has the playground's public key, so the eggs of `toolbox` must be signed by it. `hello` has no build, so nothing is packed or signed for it. Its tag is its release, and a registry with a key refuses a version that has only a tag. The line says `unsigned-tags=#true` to allow it. Anyone who can push a tag to the repository can then release `hello`, so leave the property out for a registry whose packages all publish signed eggs.

## Install and update

A tool passes its name and its facts. Here the command line plays the tool `crow` at version 1.2.0.

```bash
hatchery list --offered --target crow --fact crow=1.2.0
hatchery add example/toolbox@1.0.0 --target crow --fact crow=1.2.0 --dir plugins
```

`add` installs `toolbox` and its dependency `hello`. Look at what it wrote:

```bash
hatchery list --dir plugins
cat plugins/eggs.lock
```

The lock file records the tag, the commit and the checksum of each package. Now see what an update would do, and run it:

```bash
hatchery list --dir plugins --offered --target crow --fact crow=1.2.0
hatchery update --dir plugins --target crow --fact crow=1.2.0
```

A package that another one needs cannot be removed:

```bash
hatchery remove example/hello --dir plugins
```

## Requirements and variants

The `rookery` target of `toolbox` requires `rookery >=0.6` and Linux. A tool that is too old is refused with the reason:

```bash
hatchery add example/toolbox --target rookery --fact os=linux --fact rookery=0.5.0 --dir host
```

With a newer tool it installs, together with the variant for this machine's architecture:

```bash
hatchery add example/toolbox --target rookery --fact os=linux --fact rookery=0.7.0 --dir host
ls host/example/toolbox/rookery/variants
```

## Install from a lock

Copy a lock file to an empty folder and install exactly what it lists:

```bash
mkdir fresh && cp plugins/eggs.lock fresh/
hatchery install --dir fresh --target crow --fact crow=1.2.0
```

## Eggs as files

Download a package as one fat egg, with all its variants inside, and look at it:

```bash
hatchery fetch example/toolbox@1.1.0 --fat --out offline
hatchery inspect offline/toolbox-1.1.0.egg
```

## One folder of a repository

A package can be installed from a folder of a repository at a branch, without the repository being a registry:

```bash
hatchery add --git "$PWD/aviary.git" --path packages/hello --ref main \
  --target crow --fact crow=1.2.0 --dir oneoff
```

## Release a version

Work in the checkout. Change something in a package, then raise the version in `packages.kdl`, commit, tag and push:

```bash
cd aviary
sed -i 's/version "1.1.0"/version "1.2.0"/' packages.kdl
git commit -am "Release 1.2.0" && git tag v1.2.0 && git push origin main v1.2.0
```

`hello` is released by that tag. `toolbox` has a build, so its eggs are packed and published to the `eggs` branch:

```bash
hatchery pack packages/toolbox --out dist
hatchery publish dist/toolbox-1.2.0*.egg --sign-key-env HATCHERY_SIGN_KEY
cd ..
hatchery update --dir plugins --target crow --fact crow=1.2.0
```

Leave out the `pack` and `publish` step to see what a missing build looks like. `hatchery add example/toolbox@1.2.0 --target rookery …` then fails, because the target needs a build and no egg was published. Without the version, the newest release that has eggs is installed.

## Things to break

| Try | What happens |
| --- | --- |
| Add `min-age="3d"` to the line in `hatchery.kdl`, then `add` into a new folder | Versions are held, and `list --offered` shows until when. `--allow-young` installs anyway |
| Add `withdrawn toolbox "1.1.0"` to `release` in `aviary/packages.kdl`, commit and push | 1.1.0 is skipped when resolving. Where it is installed it stays, `list` marks it `(withdrawn)`, and `update` warns about it |
| Move a tag: `git -C aviary tag -f v1.1.0 && git -C aviary push -f origin v1.1.0` after a new commit | An install from a lock that has the old commit is refused |
| Change `key=` in `hatchery.kdl` to another public key from `hatchery keygen` | The eggs of `toolbox` are refused, because their signature does not match. `hello` still installs, from its tag, because the line allows unsigned tags |
| Remove `unsigned-tags=#true` from the line in `hatchery.kdl` | `hello` is refused, because it has only a tag and the registry requires signed eggs. `toolbox` still installs for `rookery` from its signed egg. For `crow` it needs `hello`, so it fails too |
| Edit a file under `plugins/`, then run `install` | The folder is as you left it. `install` checks eggs when it unpacks them, not folders afterwards |

## Check the playground

`examples/playground-check.sh` runs the steps above and fails if one behaves differently. CI runs it. It moves a tag at the end, so make a new playground after it.
