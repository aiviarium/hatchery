//! A blocking wrapper around [`crate::Client`], for callers without an async
//! runtime.
//!
//! Each call runs the async method with [`crate::block_on`], a minimal
//! executor. The [`Fetch`](crate::Fetch) the client was built with must not
//! need an async runtime of its own, such as one that spawns onto tokio; the
//! `http` feature's `HttpFetch` and any blocking client are fine.

use std::path::{Path, PathBuf};

use crate::block_on;
use crate::client::{Fetched, Installed, Offered, Options, Report};
use crate::error::Result;
use crate::name::{Name, Spec};
use crate::platform::Platform;

/// The same methods as [`crate::Client`], each blocking until it is done.
pub struct Client {
    inner: crate::Client,
}

impl Client {
    pub fn new(inner: crate::Client) -> Client {
        Client { inner }
    }

    /// The async client this wraps.
    pub fn inner(&self) -> &crate::Client {
        &self.inner
    }

    pub fn offered(&self, dir: &Path) -> Result<Vec<Offered>> {
        block_on(self.inner.offered(dir))
    }

    pub fn add(&self, spec: &Spec, dir: &Path) -> Result<Report> {
        block_on(self.inner.add(spec, dir))
    }

    pub fn add_with(&self, spec: &Spec, dir: &Path, opts: &Options) -> Result<Report> {
        block_on(self.inner.add_with(spec, dir, opts))
    }

    pub fn update(&self, name: Option<&Name>, dir: &Path) -> Result<Report> {
        block_on(self.inner.update(name, dir))
    }

    pub fn install(&self, dir: &Path) -> Result<Report> {
        block_on(self.inner.install(dir))
    }

    pub fn remove(&self, name: &Name, dir: &Path) -> Result<()> {
        self.inner.remove(name, dir)
    }

    pub fn installed(&self, dir: &Path) -> Result<Vec<Installed>> {
        self.inner.installed(dir)
    }

    pub fn variant(&self, pkg: &Installed, platform: &Platform) -> Result<PathBuf> {
        block_on(self.inner.variant(pkg, platform))
    }

    pub fn fetch(
        &self,
        spec: &Spec,
        variants: Option<&[String]>,
        opts: &Options,
    ) -> Result<Fetched> {
        block_on(self.inner.fetch(spec, variants, opts))
    }
}

impl From<crate::Client> for Client {
    fn from(inner: crate::Client) -> Client {
        Client::new(inner)
    }
}
