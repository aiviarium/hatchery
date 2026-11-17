# Decisions

What the formats left open, and what Hatchery chose. Each line is a rule you can rely on.

## Specs and ranges

- `group/id@1.4.0` with a complete version means exactly that version. Any other text after `@` is a range in Cargo's syntax.
- In manifests, ranges are in Cargo's syntax too. `"1.2"` means `^1.2`.
- An id alone resolves when exactly one group in the configured registries has it. Otherwise the error is `Ambiguous`.

## Variants

- The fallback variant has neither `os` nor `arch`. Its key is `default` and its folder is `variants/default`.
- The fallback is used only when no keyed variant matches, wherever it is listed.
- A variant key is its values joined by a dash, `os` first.

## Targets and manifests

- A target may say `manifest="plugin.json"`. That is the file `hatchery pack --check` validates. Without it, the file is `<target>.json` in the target's folder.
- Target names are ASCII letters, digits, `-` and `_`, and never `variants`. The manifest parser refuses the rest.
- Unknown nodes in a manifest, a root manifest and an index are kept when the file is written again.

## Eggs

- Archives are gzipped POSIX ustar, written by Hatchery's own writer and read by its own strict reader. They are not made with the `tar` crate.
- Entries are sorted by path and are files only, with no directory entries. Modes are 0644 or 0755, so the executable bit is kept. User and group ids are 0, names are empty, and the modification time is 0. The gzip header is fixed. The same input gives the same SHA-256.
- A base egg is `<id>-<version>.egg`:
  - `package.kdl`, the complete manifest, with the repository's defaults applied and an `origin` section. Every target `path=` and variant `path=` is removed, because inside the egg each target's files are at the default folder.
  - The files `about` names, at their relative paths.
  - `<target>/<path>` for each target's shared files.
- A variant egg is `<id>-<version>.<target>.<key>.egg`:
  - `variant.kdl`, the header, with `format 1` and a `variant` node holding `group`, `id`, `version`, `target` and the `os` and `arch` lines the variant has. The fallback variant has neither.
  - `<target>/variants/<key>/<path>` for the variant's files.
- A fat egg is the base egg's entries plus `variants/<variant egg file name>`, each the complete bytes of a variant egg. Removing those entries and writing again gives the base egg byte for byte.
- `origin.built` is `SOURCE_DATE_EPOCH` if set, else the commit time of `HEAD` when the folder is in a git checkout, else the current time. Packing the same commit twice gives the same egg.
- `origin.repository` is `about.repository`, else the `origin` remote of the checkout with any user and password removed.

## Installed layout

- `<dir>/eggs.lock` is TOML.
- `<dir>/<group>/<id>/` is the unpacked base egg: `package.kdl`, the about files and every target's shared files.
- `<dir>/<group>/<id>/<target>/variants/<key>/` is each variant that was fetched, unpacked from a variant egg or from inside a fat egg.
- `<dir>/.hatchery/seen.toml` records when this installation first saw each version in each registry.

## Static registry layout

- `<base>/index.json` is `{"packages": ["aiviarium/claude", …]}`.
- `<base>/<group>/<id>/index.json` is the package index. `<base>/<group>/<id>/` also holds a copy of the icon, the base eggs and the variant eggs.
- A version entry without `sha256` has only variant eggs published so far. It is not offered.
- Signatures are lowercase hex Ed25519 over the bytes `sign::message` builds: a fixed tag, then the package, version, file name, checksum and publish time, one on each line. Keys are `ed25519:<64 hex>`.
- Signatures cover `published`, so the publish time cannot be backdated without the key.
- An egg published again with the same bytes is a no-op. With different bytes it is an error, because versions are immutable. Variant eggs may be added to a released version later.

## Sources

- A single-package source (a folder, an egg file, one git folder) answers `names()` with its one name and `versions(name)` with its one offer.
- An offer with a commit from a git source must still resolve to that commit when fetched. If the tag points elsewhere now, the fetch fails with `TagMoved`.
- `Fetch::get` returns `Ok(None)` for a 404, so that a search over several registries can go on to the next.

## The command line

- `--variant TARGET os=… arch=…` names a target and then one or two keys. Values after `--variant` that contain `=` belong to the same use. A value without `=` starts the next one.
- `pack --check` reads the base egg, so it needs `--base` or a pack that writes it.
- `fetch` has no target, so it doesn't check requirements.
- `add`, `update` and `install` need at least one `--target`. Every `--fact` is given to every target.
- `publish` takes the signing key only from an environment variable that you name. `publish` without a destination reads `release.eggs` and the `origin` remote.
- `keygen` prints the signing key on the first line and the public key on the second.
- `verify --registry` compares the egg with the package index it finds in a static registry: its checksum, and with `--key` its signature.
- `registry add` writes `./hatchery.kdl` when no configuration file is found. If `HATCHERY_CONFIG` points at a file that does not exist yet, `registry add` creates it and other commands read an empty configuration.
