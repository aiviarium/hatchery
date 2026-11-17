# Command line

`hatchery` is one program with a command for each step. It prints plain text for people. `inspect` and `list` also take `--json` for scripts. Errors go to standard error as `error: <message>`.

| Exit status | Meaning |
| --- | --- |
| 0 | The command worked |
| 1 | The command failed. `error: <message>` says why |
| 2 | The command line is wrong, such as an unknown option or a missing argument. The usage message says what is expected |

`hatchery --version` prints `hatchery <version>`.

## Options for every command

| Option | Meaning |
| --- | --- |
| `--config PATH` | The `hatchery.kdl` to use. Without it, the file is found as [hatchery.kdl](hatchery-kdl.md#finding-the-file) says, and an empty configuration is used when there is none |

## Commands

| Command | What it does |
| --- | --- |
| [`pack`](#pack) | Pack a package folder into eggs |
| [`inspect`](#inspect) | Show what is in an egg |
| [`publish`](#publish) | Publish eggs to a registry |
| [`fetch`](#fetch) | Download a package's eggs without installing |
| [`add`](#add) | Install a package |
| [`update`](#update) | Update installed packages |
| [`install`](#install) | Install what `eggs.lock` lists |
| [`remove`](#remove) | Remove an installed package |
| [`list`](#list) | List installed packages, or what the registries offer |
| [`verify`](#verify) | Check an egg, and compare it with a registry |
| [`withdraw`](#withdraw) | Withdraw a version from a static registry |
| [`keygen`](#keygen) | Make a signing key |
| [`registry`](#registry) | Show or edit the registries |

## pack

```text
hatchery pack [DIR] [--base] [--variant TARGET KEY=VALUE…]… [--fat]
              [--no-build] [--check TARGET=SCHEMA]… [--out DIR]
```

Packs a package folder. `DIR` is `.` by default, and the eggs go to `dist` by default. Each egg written is printed with its SHA-256.

| Option | Meaning |
| --- | --- |
| `--base` | Write the base egg |
| `--variant TARGET KEY=VALUE…` | Write the variant egg for a target. The keys are `os` and `arch`, one or two per use. A target alone is the fallback variant. Repeat the option for more variants |
| `--fat` | Write one egg with the variants inside, instead of separate eggs |
| `--no-build` | Skip the build commands and pack what is there |
| `--check TARGET=SCHEMA` | Check the target's manifest file against a JSON Schema file. Repeat it for more targets |
| `--out DIR` | The folder the eggs are written to |

With neither `--base` nor `--variant`, `pack` writes the base egg and every variant egg.

Put `DIR` before `--variant`. The option reads the values after it up to three, so a folder after it is taken for a value.

`--check` reads the base egg, so pack it too. The file checked is the target's `manifest=` property, or `<target>.json`. The file comes from the egg, so a build command's output is checked. When it fails the schema, `pack` lists each error with its JSON pointer and exits with status 1 before it writes an egg.

```bash
hatchery pack agents/claude --variant rookery arch=x86_64 --out dist/
hatchery pack agents/claude --base --out dist/ \
  --check crow=schemas/crow-plugin.schema.json
```

## inspect

```text
hatchery inspect EGG [--json]
```

Shows the kind (base, variant or fat), the package and version, display data, the SHA-256 and the origin. For each target it shows requirements, dependencies, optional dependencies and variants. For a fat egg it says which variants are inside.

`--json` prints the same as one JSON object.

## publish

```text
hatchery publish EGG… [--to DIR | --git URL [--branch B] [--path P]]
                      [--sign-key-env VAR]
```

Publishes eggs. Variant eggs are published before base eggs, whatever the order given.

| Option | Meaning |
| --- | --- |
| `--to DIR` | A static registry folder |
| `--git URL` | A git repository, with eggs in the static registry layout on a branch |
| `--branch B` | The branch of `--git`. `eggs` by default |
| `--path P` | A folder of that branch |
| `--sign-key-env VAR` | The environment variable that holds the signing key |

Without `--to` or `--git`, `publish` finds `packages.kdl` from the current folder upward. It takes the branch and folder from `release.eggs` and the address from the `origin` remote of that checkout. With no `release.eggs`, it stops with an error that says to pass `--to` or `--git`.

The signing key comes only from the variable. It is never a command line argument. Each egg published is printed with its file name, SHA-256 and whether it was signed.

## fetch

```text
hatchery fetch SPEC [--git URL --path P --ref R] [--fat]
                    [--variant TARGET.KEY]… [--out DIR] [--allow-young]
```

Downloads a package's eggs from the registries, without installing. `SPEC` is `group/id[@range]`, an id, a folder, an egg file or an address. With `--git URL` the package is one folder of a repository, and `SPEC` is not needed.

| Option | Meaning |
| --- | --- |
| `--git URL`, `--path P`, `--ref R` | One folder of a git repository at one tag, branch or commit |
| `--fat` | Write one fat egg, `<id>-<version>.egg`, instead of separate eggs |
| `--variant TARGET.KEY` | Fetch only this variant. Repeat for more. Without it, all variants are fetched |
| `--out DIR` | The folder to write to. `.` by default |
| `--allow-young` | Fetch even if the version is younger than the minimum release age |

`fetch` has no tool to answer requirements, so it checks none.

```bash
hatchery fetch aiviarium/claude@1.4.0 --fat --out ./offline
```

## add

```text
hatchery add [SPEC] [--git URL --path P --ref R] [--dir DIR]
             [--target NAME]… [--fact NAME=VALUE]… [--on-demand] [--allow-young]
```

Resolves, verifies, unpacks and locks a package and its dependencies.

| Option | Meaning |
| --- | --- |
| `--dir DIR` | The install folder. `.` by default |
| `--target NAME` | A tool to install for. At least one is required. Repeat for more |
| `--fact NAME=VALUE` | A fact every target answers, such as `crow=1.2.0`. Repeat for more |
| `--on-demand` | Skip variants at install. A tool fetches them later |
| `--allow-young` | Install even if the version is younger than the minimum release age |

Without `--on-demand`, the variant for this machine's OS and architecture is installed.

When a newer version in the range cannot be installed here, `add` installs the newest one that can. It then prints a warning with the newer version and the reason, such as a missing built egg or a withdrawal.

The base egg is unpacked whole, so the shared files of the package's other targets are in the folder too. Only the targets of `--target` are checked, locked and given their variants.

```bash
hatchery add aiviarium/claude@^1.4 --target crow --fact crow=1.2.0 --dir ./plugins
hatchery add --git https://github.com/aiviarium/parrot.git --path agents/claude --ref v1.4.0 --target rookery
```

## update

```text
hatchery update [NAME] [--dir DIR] [--target NAME]… [--fact NAME=VALUE]… [--on-demand]
```

Updates one package, or all, to the newest allowed versions.

## install

```text
hatchery install [--dir DIR] [--target NAME]… [--fact NAME=VALUE]… [--on-demand]
```

Installs exactly what `eggs.lock` lists, and checks checksums and commits. A package is left alone when its folder already contains the locked version, unpacked from the locked egg.

## remove

```text
hatchery remove NAME [--dir DIR]
```

Removes a package. It refuses while another installed package depends on it.

## list

```text
hatchery list [--dir DIR] [--offered] [--json]
```

Lists installed packages with version, targets and source. The version is the one in the package's folder. A version its registry has withdrawn since is marked `(withdrawn)`, with the reason when there is one, so `list` reads the registries. A registry it cannot read is skipped.

With `--offered`, lists what the registries offer. Each row has the newest version, the installed version, the version an update would install and the time a held version becomes installable. A withdrawn version is marked `(withdrawn)`. A package a registry had to skip, such as one whose `package.kdl` does not parse, is a warning. `--target` and `--fact` also work here, so that requirements are checked.

With `--json`, an installed package has a `withdrawn` field, `null` or `{"reason": …}`.

## verify

```text
hatchery verify EGG [--registry DIR_OR_URL] [--key ed25519:…]
```

Opens the egg, which refuses unsafe entries, and prints its SHA-256.

With `--registry`, looks the version up in that static registry's index. It checks that the egg's checksum is the one listed. With `--key`, it also checks the signature. A fat egg is checked as the base egg it was made from.

## withdraw

```text
hatchery withdraw NAME@VERSION --to DIR [--reason TEXT]
```

Marks a published version of a static registry folder as withdrawn. For a git registry, add a `withdrawn` line to `packages.kdl`. See [Registries](registries.md#withdrawing-a-version).

## keygen

```text
hatchery keygen
```

Makes a signing key. Standard output has two lines: the signing key, then the public key. A note on standard error says what each is for. Only the signing key is secret.

```bash
hatchery keygen > keys.txt
```

Store the first line as a CI secret and give the second to readers as `key=`.

## registry

```text
hatchery registry list
hatchery registry add KIND LOCATION [--only G,…] [--key K] [--min-age D] [--token-env VAR]
hatchery registry remove LOCATION
```

Edits the configuration file in place and keeps its comments and layout. `KIND` is `git`, `http` or `dir`. If no configuration file is found, `add` creates `./hatchery.kdl`.

## A CI pipeline

Packing, checking and publishing with no other tool. The signing key is in the secret `HATCHERY_SIGN_KEY`.

On a tag, in each build job (here x86_64 and aarch64), publish that job's variant egg:

```bash
hatchery pack agents/claude --variant rookery arch=$ARCH --out dist/
hatchery publish dist/*.egg --sign-key-env HATCHERY_SIGN_KEY
```

In the last job, pack the base egg, which makes the version visible, check it and publish it:

```bash
hatchery pack agents/claude --base --out dist/ \
  --check crow=schemas/crow-plugin.schema.json \
  --check rookery=schemas/rookery-plugin.schema.json

hatchery inspect dist/claude-1.4.0.egg
hatchery publish dist/claude-1.4.0.egg --sign-key-env HATCHERY_SIGN_KEY
```

In the first job `dist` has only that job's variant egg. The `publish` in the last job reads `release.eggs` from `packages.kdl`, so it needs a checkout with an `origin` remote it can push to.

A package with no built target has no CI step. The tag is the release.
