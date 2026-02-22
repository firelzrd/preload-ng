#![forbid(unsafe_code)]

use slotmap::new_key_type;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fmt, hash};

new_key_type! { pub struct ExeId; }
new_key_type! { pub struct MapId; }

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExeKey(Arc<Path>);

impl ExeKey {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(Arc::from(path.into().as_path()))
    }

    pub fn from_arc(path: Arc<Path>) -> Self {
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn arc_path(&self) -> &Arc<Path> {
        &self.0
    }
}

impl hash::Hash for ExeKey {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Debug for ExeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ExeKey").field(&self.0).finish()
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MapKey {
    pub path: Arc<Path>,
    pub offset: u64,
    pub length: u64,
}

impl MapKey {
    pub fn new(path: impl Into<PathBuf>, offset: u64, length: u64) -> Self {
        Self {
            path: Arc::from(path.into().as_path()),
            offset,
            length,
        }
    }

    pub fn from_arc(path: Arc<Path>, offset: u64, length: u64) -> Self {
        Self {
            path,
            offset,
            length,
        }
    }
}

impl hash::Hash for MapKey {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.offset.hash(state);
        self.length.hash(state);
    }
}

impl fmt::Debug for MapKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MapKey")
            .field("path", &self.path)
            .field("offset", &self.offset)
            .field("length", &self.length)
            .finish()
    }
}
