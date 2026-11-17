//! A package folder, for development, and an egg file, by path or URL.
//! Neither is held by the minimum release age: you chose the exact one.

use std::path::PathBuf;
use std::sync::Arc;

use crate::fetch::Fetch;
use crate::target::Target;

/// A package folder, packed as it is, without a build.
pub struct FolderSource {
    _private: (),
}

impl FolderSource {
    /// `targets` are asked to `derive` a manifest when the folder has no
    /// `package.kdl`.
    pub fn new(dir: impl Into<PathBuf>, targets: Vec<Arc<dyn Target>>) -> FolderSource {
        let _ = (dir.into(), targets);
        todo!()
    }
}

/// An egg file on disk or at an `http(s)://` address. A fat egg needs no
/// registry for its variants.
pub struct FileSource {
    _private: (),
}

impl FileSource {
    pub fn new(location: impl Into<String>, fetch: Option<Arc<dyn Fetch>>) -> FileSource {
        let _ = (location.into(), fetch);
        todo!()
    }
}
