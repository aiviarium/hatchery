//! The command line grammar.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "hatchery",
    version,
    about = "Pack, publish and install eggs: packages with content for several tools"
)]
pub struct Cli {
    /// The hatchery.kdl to use instead of the one that is found
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Pack a package folder into eggs
    Pack(Pack),
    /// Show what an egg holds
    Inspect(Inspect),
    /// Publish eggs to a registry
    Publish(Publish),
    /// Download a package's eggs without installing
    Fetch(Fetch),
    /// Install a package
    Add(Add),
    /// Update installed packages to the newest allowed versions
    Update(Update),
    /// Install what eggs.lock lists
    Install(Install),
    /// Remove an installed package
    Remove(Remove),
    /// List installed packages, or what the registries offer
    List(List),
    /// Check an egg, and compare it with a registry's index
    Verify(Verify),
    /// Mark a published version of a static registry as withdrawn
    Withdraw(Withdraw),
    /// Make a signing key
    Keygen,
    /// Show or edit the registries in hatchery.kdl
    #[command(subcommand)]
    Registry(RegistryCmd),
}

#[derive(Args, Debug)]
pub struct Pack {
    /// The package folder
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// Write the base egg
    #[arg(long)]
    pub base: bool,
    /// Write the variant egg for a target. Takes the target, then os= and
    /// arch= values, such as `--variant rookery arch=x86_64`. Put DIR before it.
    #[arg(long, num_args = 1..=3, value_names = ["TARGET", "KEY=VALUE"])]
    pub variant: Vec<String>,
    /// Write one egg with the variants inside
    #[arg(long)]
    pub fat: bool,
    /// Pack what is there without running build commands
    #[arg(long)]
    pub no_build: bool,
    /// Check a target's manifest file against a JSON Schema file, as TARGET=SCHEMA
    #[arg(long, value_name = "TARGET=SCHEMA")]
    pub check: Vec<String>,
    /// The folder the eggs are written to
    #[arg(long, default_value = "dist")]
    pub out: PathBuf,
}

#[derive(Args, Debug)]
pub struct Inspect {
    pub egg: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct Publish {
    #[arg(required = true)]
    pub eggs: Vec<PathBuf>,
    /// A static registry folder
    #[arg(long, conflicts_with = "git")]
    pub to: Option<PathBuf>,
    /// A git repository to publish to
    #[arg(long, value_name = "URL")]
    pub git: Option<String>,
    /// The branch of --git (default eggs)
    #[arg(long, requires = "git")]
    pub branch: Option<String>,
    /// A folder of the branch
    #[arg(long, requires = "git")]
    pub path: Option<String>,
    /// The environment variable that holds the signing key
    #[arg(long, value_name = "VAR")]
    pub sign_key_env: Option<String>,
}

#[derive(Args, Debug)]
pub struct Source {
    /// The package: group/id[@range], a folder, an egg file or an https address
    #[arg(required_unless_present = "git")]
    pub spec: Option<String>,
    /// One folder of a git repository instead of a registry
    #[arg(long, value_name = "URL")]
    pub git: Option<String>,
    /// The package's folder in the repository
    #[arg(long, requires = "git")]
    pub path: Option<String>,
    /// A tag, branch or commit of the repository
    #[arg(long = "ref", requires = "git")]
    pub git_ref: Option<String>,
}

#[derive(Args, Debug)]
pub struct Fetch {
    #[command(flatten)]
    pub source: Source,
    /// Write one egg with the variants inside
    #[arg(long)]
    pub fat: bool,
    /// Fetch only this variant, as TARGET.KEY (all when not given)
    #[arg(long, value_name = "TARGET.KEY")]
    pub variant: Vec<String>,
    #[arg(long, default_value = ".")]
    pub out: PathBuf,
    /// Fetch even if the version is younger than the minimum release age
    #[arg(long)]
    pub allow_young: bool,
}

/// The tools an install is for.
#[derive(Args, Debug)]
pub struct Tools {
    /// A target to install for, such as crow. Repeat it for more.
    #[arg(long = "target", value_name = "NAME")]
    pub targets: Vec<String>,
    /// A fact every target answers, as NAME=VALUE
    #[arg(long = "fact", value_name = "NAME=VALUE")]
    pub facts: Vec<String>,
    /// Do not fetch variants at install
    #[arg(long)]
    pub on_demand: bool,
}

#[derive(Args, Debug)]
pub struct Add {
    #[command(flatten)]
    pub source: Source,
    /// The install folder
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
    #[command(flatten)]
    pub tools: Tools,
    /// Install even if the version is younger than the minimum release age
    #[arg(long)]
    pub allow_young: bool,
}

#[derive(Args, Debug)]
pub struct Update {
    /// One package, group/id; all when not given
    pub name: Option<String>,
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
    #[command(flatten)]
    pub tools: Tools,
}

#[derive(Args, Debug)]
pub struct Install {
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
    #[command(flatten)]
    pub tools: Tools,
}

#[derive(Args, Debug)]
pub struct Remove {
    /// group/id
    pub name: String,
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
}

#[derive(Args, Debug)]
pub struct List {
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
    /// List what the registries offer
    #[arg(long)]
    pub offered: bool,
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub tools: Tools,
}

#[derive(Args, Debug)]
pub struct Verify {
    pub egg: PathBuf,
    /// A static registry, a folder or an https address
    #[arg(long, value_name = "DIR_OR_URL")]
    pub registry: Option<String>,
    /// The registry's public key, ed25519:…
    #[arg(long, requires = "registry")]
    pub key: Option<String>,
}

#[derive(Args, Debug)]
pub struct Withdraw {
    /// group/id@version
    pub name: String,
    /// The static registry folder
    #[arg(long)]
    pub to: PathBuf,
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum RegistryCmd {
    /// List the registries in the configuration
    List,
    /// Add a registry
    Add(RegistryAdd),
    /// Remove a registry by its location
    Remove { location: String },
}

#[derive(Args, Debug)]
pub struct RegistryAdd {
    /// git, http or dir
    pub kind: String,
    /// A repository address, a base URL or a folder
    pub location: String,
    /// Serve only these groups, comma separated
    #[arg(long, value_name = "G,…")]
    pub only: Option<String>,
    /// The registry's public key
    #[arg(long, value_name = "ed25519:…")]
    pub key: Option<String>,
    /// The minimum release age, such as 3d
    #[arg(long, value_name = "D")]
    pub min_age: Option<String>,
    /// The environment variable that holds an access token
    #[arg(long, value_name = "VAR")]
    pub token_env: Option<String>,
}
