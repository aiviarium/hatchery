//! Hatchery packs package folders into eggs, publishes them, resolves
//! versions, verifies and installs. A package carries content for several
//! tools (targets). Each tool reads its own target's folder, and Hatchery
//! checks, locks and fetches variants only for the client's targets. The
//! base egg is unpacked whole, so the shared files of the other targets are
//! in the package folder too.
//!
//! The surface is one [`Client`], one trait per extension point
//! ([`Target`], [`Source`], [`Fetch`]) and plain data types.

// Errors carry the package, version and registry so tools can show them;
// they are not on a hot path, so their size does not matter.
#![allow(clippy::result_large_err)]
// A library reports through its return values. Printing is the caller's.
#![warn(clippy::print_stdout, clippy::print_stderr)]

pub mod archive;
#[cfg(feature = "blocking")]
pub mod blocking;
pub mod client;
pub mod config;
pub mod digest;
pub mod egg;
pub mod error;
mod exec;
pub mod fetch;
#[cfg(feature = "http")]
mod fetch_http;
pub mod fsx;
pub mod git;
pub mod glob;
pub mod index;
mod kdlx;
pub mod lock;
pub mod manifest;
pub mod name;
pub mod pack;
pub mod platform;
pub mod publish;
pub mod repo;
pub mod resolve;
pub mod seen;
pub mod sign;
pub mod source;
pub mod target;
pub mod time;

pub use bytes::Bytes;
pub use kdl;
pub use semver;

pub use client::{
    Change, Client, ClientBuilder, Installed, Listing, Offered, OfferedVersion, Options, Report,
};
pub use config::{Config, RegistryConfig};
pub use digest::{sha256_hex, tree_digest};
pub use egg::Egg;
pub use error::{At, Error, Result};
pub use exec::block_on;
pub use fetch::{BoxFuture, Fetch, Token, Url};
pub use git::Git;
pub use lock::{Lock, LockedPackage};
pub use manifest::{About, Manifest, Requirement, TargetSpec, VariantSpec};
pub use name::{Name, Spec, Wanted};
pub use pack::{PackOptions, VariantPick, pack};
pub use platform::{Platform, Variants};
pub use publish::{PublishTo, Published, publish};
pub use sign::{SigningKey, VerifyingKey};
pub use source::{Details, Offer, Source};
pub use target::{Problem, StaticTarget, Target, TargetFiles};
pub use time::Timestamp;

/// This crate's version, written into the origin of every egg it packs.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
