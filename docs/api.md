# Rust API

The `hatchery` crate is one `Client`, one trait for each extension point, and plain data types. A tool adapts Hatchery with its own versions of the traits.

```toml
[dependencies]
hatchery = { git = "https://github.com/aiviarium/hatchery", tag = "v0.1.0", features = ["http"] }
```

| Feature | Adds |
| --- | --- |
| `http` | `hatchery::fetch::HttpFetch`, a `Fetch` over HTTP(S) with ureq. Without it, you bring your own |
| `blocking` | `hatchery::blocking`, a blocking wrapper around `Client` |

Both are off by default. The library has no async runtime. `hatchery::block_on` runs a future to completion on the current thread.

The futures block the thread they run on. They run `git` and wait for it, and `HttpFetch` waits for ureq. An async caller runs each client call on a thread for blocking work, such as `tokio::task::spawn_blocking`, and drives it there with `block_on`. A `Fetch` of your own that does not block doesn't help with a git registry, since git still blocks.

The trait methods return `BoxFuture` from `hatchery::fetch` rather than being `async fn`, so `Source` and `Fetch` work as trait objects. A source or fetcher of your own returns `Box::pin(async move { … })`.

## A usage example

```rust
use std::path::Path;

use hatchery::fetch::HttpFetch;
use hatchery::{Client, Config, Spec, StaticTarget, block_on};

fn install(dir: &Path) -> hatchery::Result<()> {
    let config = Config::load(Path::new("hatchery.kdl"))?;
    let client = Client::builder()
        .config(config)
        .target(StaticTarget::new("crow").fact("crow", "1.2.0"))
        .fetch(HttpFetch::new())
        .build()?;

    let spec = Spec::parse("aiviarium/claude@^1.4")?;
    let report = block_on(client.add(&spec, dir))?;
    for change in &report.packages {
        println!("{} {}", change.name, change.version);
    }
    Ok(())
}
```

## Target

What a tool tells Hatchery about itself.

```rust
pub trait Target: Send + Sync {
    fn name(&self) -> &str;
    fn fact(&self, name: &str) -> Option<String>;
    fn variants(&self) -> Variants { /* the variant for this machine */ }
    fn check(&self, files: &TargetFiles) -> Result<(), Vec<Problem>> { Ok(()) }
    fn derive(&self, dir: &Path) -> Result<Option<Manifest>> { Ok(None) }
}
```

| Method | Meaning |
| --- | --- |
| `name` | The name under `targets` this tool installs, such as `crow` |
| `fact` | Answers a name from a `requires` block. `None` means it is absent. `os` and `arch` fall back to the platform of `variants`, or this machine |
| `variants` | `Variants::Matching(Platform)` fetches the variant for that platform at install, and is the default with `Platform::current()`. `Variants::OnDemand` fetches none, and `Client::variant` fetches each when it is first needed |
| `check` | Called at pack and at install with the target's files. It reads the tool's own manifest and returns a list of `Problem`s |
| `derive` | For a folder without a `package.kdl`: derive one from the tool's own manifest |

`TargetFiles` holds the files of one target by path, relative to the target's folder. The `variants` folder is not part of it. `StaticTarget` is a `Target` that answers from a fixed table. The `hatchery` command uses it for `--target` and `--fact`.

## Source

Where packages come from. Git repositories, static registries, single git folders, folders and files are built in.

```rust
pub trait Source: Send + Sync {
    fn label(&self) -> &str;
    fn is_local(&self) -> bool { false }
    fn warnings(&self) -> Vec<String> { Vec::new() }
    fn names(&self) -> BoxFuture<'_, Result<Vec<Name>>>;
    fn versions<'a>(&'a self, name: &'a Name) -> BoxFuture<'a, Result<Vec<Offer>>>;
    fn details<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Details>>;
    fn fetch<'a>(&'a self, offer: &'a Offer) -> BoxFuture<'a, Result<Egg>>;
    fn fetch_variant<'a>(&'a self, offer: &'a Offer, target: &'a str, key: &'a str)
        -> BoxFuture<'a, Result<Egg>>;
}
```

`names`, `details`, `fetch_variant` and `warnings` have default bodies. A source of one package, such as a folder, an egg file or one git folder, answers `names` with that package and `versions` with its one version. `warnings` reports what the source skipped, such as a package whose `package.kdl` it cannot read. The client adds them to its reports and listings. `Source::fetch` returns an egg whose checksum and signature the source has verified, and whose manifest names the offered package and version.

A tool adds a kind of registry with `ClientBuilder::kind(name, factory)`. A registry line of that node name is then served by the factory's source.

| Item | Meaning |
| --- | --- |
| `Offer` | One version a source offers, with its registry label, times, checksum and signature, and for git its commit, ref and path. `needs_signature` marks a version of a keyed registry that has only a tag. It is never installed |
| `Details` | Display data and the targets of a version, so that resolving a version can skip the egg |
| `StaticRegistry` | A static registry, from `StaticRegistry::dir`, `::http` or `::new` over any `RegistryFiles` |
| `GitRegistry`, `GitFolder`, `FolderSource`, `FileSource` | The other built-in sources. `GitRegistry::unsigned_tags(true)` lets a keyed registry install tag-only versions |

## Fetch

How bytes are fetched over HTTP. The tool passes its own client.

```rust
pub trait Fetch: Send + Sync {
    fn get<'a>(&'a self, url: &'a Url, auth: Option<&'a Token>)
        -> BoxFuture<'a, Result<Option<Bytes>>>;
}
```

A 404 is `Ok(None)` so that search order works. Any other failure is `Error::Http`. `Token` is a bearer token. `Token::from_env` reads it from a variable, and `Debug` doesn't show it.

## Client

Built with `Client::builder()`:

| Builder method | Meaning |
| --- | --- |
| `config(Config)` | The registries and the default minimum release age |
| `target(impl Target)`, `target_arc` | A tool this client installs for. Call it once per target. A package must have at least one of them. Requirements, dependencies and variants are read for those targets only |
| `fetch(impl Fetch)`, `fetch_arc` | How HTTP registries and egg addresses are fetched. Without one, they fail |
| `cache_dir(path)` | Where git checkouts are cached. `$HATCHERY_CACHE` by default, or `hatchery` in the user's cache folder when it is not set |
| `clock(Clock)` | The time source, for tests |
| `git(Git)` | The `git` program to run |
| `kind(name, factory)` | Registry lines of this node name |
| `build()` | Returns the `Client` |

| Client method | Meaning |
| --- | --- |
| `offered(dir).await` | A `Listing`: the packages the registries offer, each with its about data, versions, installed version and available update, and when a held version can be installed. Its `warnings` say what a registry skipped |
| `add(spec, dir).await`, `add_with(spec, dir, &Options)` | Resolves, verifies, unpacks and locks a package and its dependencies. `Options::allow_young` skips the minimum release age for the package you add |
| `update(name, dir).await` | Updates one package, or all with `None` |
| `install(dir).await` | Installs exactly what `eggs.lock` lists |
| `remove(name, dir)` | Removes a package. Refuses while a dependent is installed |
| `installed(dir)` | The packages in a folder, as `Installed`. Its version is the one in the package's folder |
| `withdrawn(dir).await` | The installed packages whose version their registry has withdrawn since, with the reason |
| `variant(&Installed, &Platform).await` | The files of one variant. Downloads and verifies its egg on first use |
| `fetch(spec, variants, &Options).await` | Fetches the base egg and variant eggs without installing. `variants` are named `<target>.<key>`, all when `None` |

`add`, `update` and `install` return a `Report`. It has the folder and the `Change`s, with dependencies first. It also has warnings, such as a withdrawn version that stays installed. A `Change` has the name, the version, the version before, the SHA-256, the registry label and the `Egg`.

A `Spec` is parsed from text with `Spec::parse`: `Registry(Wanted)`, `Git { url, path, git_ref }`, `Folder` or `File`. A `Name` is `group/id`.

## Egg

An opened archive. `Egg::open` checks each entry.

| Method | Meaning |
| --- | --- |
| `Egg::open(bytes)`, `Egg::read(path)` | Opens an egg of any kind. Refuses unsafe entries |
| `Egg::pack(dir, targets)` | Packs a folder into a base egg without a build, checking each target |
| `Egg::from_entries(entries)` | Builds an egg from `Entry` values, written deterministically. `Entry::new(path, data)` makes one |
| `manifest()` | The package, about, targets and origin |
| `variant()` | The header of a variant egg, `None` for others |
| `is_fat()` | Whether variant eggs are inside |
| `target(name)` | The shared files of one target, as `TargetFiles` |
| `subset(&[targets])` | The same egg with only those targets. `None` when it has none of them |
| `sha256()`, `bytes()`, `file_name()` | The checksum, the archive and the egg's name |
| `embedded_variants()`, `embedded_variant(target, key)` | The variant eggs inside a fat egg |
| `base()` | A fat egg without its variants, byte for byte the base egg it was made from |
| `fatten(&[variants])` | A fat egg made of this base egg and the variant eggs you pass |
| `unpack(dest)` | Unpacks into a package folder |

`hatchery::pack(dir, &PackOptions)` is `hatchery pack`. It returns the variant eggs first and the base or fat egg last. `PackOptions::new()` starts the options, and its methods set them: `base`, `variant(VariantPick)`, `fat`, `no_build` and `check`. `defaults`, `origin` and `now` replace what is otherwise read from `packages.kdl`, git and the clock. `git(Git)` sets how `pack` runs git.

```rust
let opts = PackOptions::new()
    .no_build(true)
    .variant(VariantPick::new("rookery").os("linux").arch("x86_64"));
let eggs = hatchery::pack(dir, &opts)?;
```

## Publishing and tree_digest

```rust
pub fn publish(egg: &Egg, to: &PublishTo, key: Option<&SigningKey>) -> Result<Published>;
pub fn tree_digest(dir: &Path) -> Result<String>;
```

`PublishTo::Dir(path)` publishes into a static registry folder. `PublishTo::Git { url, branch, path }` publishes to a branch of a git repository, creating the branch when it does not exist. `Published` reports the file name, SHA-256, whether it was signed and the publish time. `publish_at` takes the time as an argument. `publish::withdraw_dir` marks a version withdrawn.

`tree_digest` is a checksum of a folder's files. It covers the paths, the executable bit of each file and the contents. The same files give the same digest on any machine. A tool can compare the digest of an installed package with what a host applied.

## Signing

The `sign` module has `parse_signing_key`, `parse_verifying_key`, `format_signing_key`, `format_verifying_key`, `message`, `sign` and `verify`. Keys are written `ed25519:` and 64 hex digits.

`message` builds the bytes that are signed: a fixed tag, then the package, version, file name, SHA-256 and publish time, one on each line. The signature is lowercase hex.

## Data types

Resolution, the git cache and the archive writer are private, so a minor release can change them. The public API is the client, the traits, the sources, the errors and the data types below.

The structs with public fields and the enums other than `Error` are marked `non_exhaustive`, so a later version can add a field or a variant. Read their fields and match with `..`. To make one, use its constructor or its builder:

| Type | Made with |
| --- | --- |
| `PackOptions`, `VariantPick`, `Options`, `Origin` | `new()` and setter methods |
| `Offer`, `Details`, `TargetInfo`, `VariantInfo`, `Withdrawn` | `new(…)`, then set the public fields. For a `Source` of your own |
| `Problem`, `TargetFiles`, `Manifest`, `Requirement`, `VariantSpec`, `Dependency` | `new(…)`. For a `Target` of your own |
| `Entry` | `Entry::new(path, data)`, and `.executable(true)` |
| `Name`, `Spec`, `Config`, `RegistryConfig` | `Name::new` and `parse`, `Spec::parse` and `Spec::git`, `Config::load` and `parse`, `RegistryConfig::new` |

## Errors

`hatchery::Error` is one enum, marked `non_exhaustive`. Match on the variants below. Each variant that gives a reason a package cannot be installed has an `At` with the package, version and registry, and its `Display` text is a sentence a person can read. `Error::at()` returns the `At` when there is one.

| Variant | Meaning |
| --- | --- |
| `ChecksumMismatch` | The bytes do not have the checksum the index or the lock file gives |
| `SignatureInvalid` | The registry has a key and the egg is not signed by it |
| `Unmet` | A line of a target's `requires` block does not hold. Names the target, the fact, what was wanted and what was found |
| `NoVariant` | The target declares variants and none is for this platform |
| `TooYoung` | The version is younger than the minimum release age. Carries the time it becomes installable |
| `Withdrawn` | The version was pulled, with an optional reason |
| `NotBuilt` | A git registry has no built egg and a wanted target needs a build |
| `TagMoved` | A tag points at a different commit than the lock file records |
| `RequiredBy` | A package cannot be removed while dependents are installed |
| `NotFound` | No registry has the package, or no version in the range |
| `Ambiguous` | An id alone matches packages of several groups |
| `Conflict` | No set of versions fits all the ranges |
| `Invalid` | A manifest, configuration, lock file or index is not valid |
| `UnsafeArchive` | An egg has an entry that unpacking refuses |
| `Check` | A target's own check refused its files, with the list of `Problem`s |
| `Build` | A build command failed |
| `Git`, `Http`, `Io`, `Other` | A failed `git` command, a failed fetch, an I/O error with its path, and anything else |
