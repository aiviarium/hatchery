//! Static registries: the layout of index files and eggs, read from a
//! folder, over HTTP, or from a branch of a git repository.

use std::path::PathBuf;
use std::sync::Arc;

use crate::fetch::{Fetch, Token, Url};
use crate::sign::VerifyingKey;
use crate::source::RegistryFiles;

/// A static registry over any [`RegistryFiles`].
pub struct StaticRegistry {
    _private: (),
}

impl StaticRegistry {
    pub fn new(files: Arc<dyn RegistryFiles>, key: Option<VerifyingKey>) -> StaticRegistry {
        let _ = (files, key);
        todo!()
    }

    /// A registry folder.
    pub fn dir(path: impl Into<PathBuf>, key: Option<VerifyingKey>) -> StaticRegistry {
        StaticRegistry::new(Arc::new(DirFiles::new(path)), key)
    }

    /// A registry over HTTP.
    pub fn http(
        base: Url,
        fetch: Arc<dyn Fetch>,
        token: Option<Token>,
        key: Option<VerifyingKey>,
    ) -> StaticRegistry {
        StaticRegistry::new(Arc::new(HttpFiles::new(base, fetch, token)), key)
    }
}

/// The files of a registry folder.
pub struct DirFiles {
    _private: (),
}

impl DirFiles {
    pub fn new(path: impl Into<PathBuf>) -> DirFiles {
        let _ = path.into();
        todo!()
    }
}

/// The files of a registry over HTTP.
pub struct HttpFiles {
    _private: (),
}

impl HttpFiles {
    pub fn new(base: Url, fetch: Arc<dyn Fetch>, token: Option<Token>) -> HttpFiles {
        let _ = (base, fetch, token);
        todo!()
    }
}

impl RegistryFiles for DirFiles {
    fn label(&self) -> &str {
        todo!()
    }

    fn read<'a>(
        &'a self,
        path: &'a str,
    ) -> crate::fetch::BoxFuture<'a, crate::error::Result<Option<bytes::Bytes>>> {
        let _ = path;
        todo!()
    }
}

impl RegistryFiles for HttpFiles {
    fn label(&self) -> &str {
        todo!()
    }

    fn read<'a>(
        &'a self,
        path: &'a str,
    ) -> crate::fetch::BoxFuture<'a, crate::error::Result<Option<bytes::Bytes>>> {
        let _ = path;
        todo!()
    }
}
