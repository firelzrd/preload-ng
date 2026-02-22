#![forbid(unsafe_code)]

use super::MapKey;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapSegment {
    pub path: Arc<Path>,
    pub offset: u64,
    pub length: u64,
    pub update_time: u64,
    /// Device number (from procfs dev major/minor or stat st_dev). 0 = unknown.
    pub device: u64,
    /// Inode number. 0 = unknown.
    pub inode: u64,
}

impl MapSegment {
    pub fn new(path: impl Into<PathBuf>, offset: u64, length: u64, update_time: u64) -> Self {
        Self {
            path: Arc::from(path.into().as_path()),
            offset,
            length,
            update_time,
            device: 0,
            inode: 0,
        }
    }

    pub fn from_arc(path: Arc<Path>, offset: u64, length: u64, update_time: u64) -> Self {
        Self {
            path,
            offset,
            length,
            update_time,
            device: 0,
            inode: 0,
        }
    }

    pub fn key(&self) -> MapKey {
        MapKey::from_arc(self.path.clone(), self.offset, self.length)
    }
}
