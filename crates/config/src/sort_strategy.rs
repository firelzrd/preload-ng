use serde::{Deserialize, Serialize};

/// The I/O sorting strategy.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortStrategy {
    /// Sort based on file path only. Useful for network filesystems.
    #[serde(rename = "path")]
    Path,

    /// Sort based on inode number. Does less house-keeping I/O than the next
    /// option.
    #[serde(rename = "inode")]
    Inode,

    /// Sort I/O based on disk block. Most sophisticated. And useful for most
    /// Linux filesystems.
    #[serde(rename = "block")]
    Block,
}
