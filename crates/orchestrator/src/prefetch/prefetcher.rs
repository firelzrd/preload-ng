#![deny(unsafe_code)]

use crate::prefetch::{PrefetchPlan, PrefetchReport};
use crate::stores::Stores;
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use nix::fcntl::PosixFadviseAdvice;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroUsize;
use std::os::unix::fs::OpenOptionsExt;
use tracing::debug;

#[async_trait]
pub trait Prefetcher: Send + Sync {
    /// Execute the prefetch plan (side effects only).
    async fn execute(&self, plan: &PrefetchPlan, stores: &Stores) -> PrefetchReport;
}

#[derive(Debug, Default)]
pub struct NoopPrefetcher;

#[async_trait]
impl Prefetcher for NoopPrefetcher {
    async fn execute(&self, _plan: &PrefetchPlan, _stores: &Stores) -> PrefetchReport {
        PrefetchReport::default()
    }
}

// ---------------------------------------------------------------------------
// ReadPrefetcher — posix_fadvise(SEQUENTIAL) + read() loop (original method)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReadPrefetcher {
    concurrency: usize,
}

impl ReadPrefetcher {
    pub fn new(concurrency: usize) -> Self {
        Self { concurrency }
    }

    fn readahead(path: &std::path::Path, offset: i64, length: i64) -> Result<(), std::io::Error> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NOATIME)
            .open(path)?;

        let _ = nix::fcntl::posix_fadvise(
            &file,
            offset,
            length,
            PosixFadviseAdvice::POSIX_FADV_SEQUENTIAL,
        );

        if offset > 0 {
            file.seek(SeekFrom::Start(offset as u64))?;
        }
        let mut remaining = length as u64;
        let mut buf = vec![0u8; 128 * 1024];
        while remaining > 0 {
            let to_read = (remaining as usize).min(buf.len());
            let n = file.read(&mut buf[..to_read])?;
            if n == 0 {
                break;
            }
            remaining -= n as u64;
        }

        Ok(())
    }
}

#[async_trait]
impl Prefetcher for ReadPrefetcher {
    async fn execute(&self, plan: &PrefetchPlan, stores: &Stores) -> PrefetchReport {
        execute_concurrent(plan, stores, self.concurrency, |path, offset, length| {
            Self::readahead(path, offset, length)
        })
        .await
    }
}

// Backward-compatible alias.
pub type PosixFadvisePrefetcher = ReadPrefetcher;

// ---------------------------------------------------------------------------
// ReadaheadPrefetcher — readahead(2) syscall (no user-space buffer copy)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReadaheadPrefetcher {
    concurrency: usize,
}

impl ReadaheadPrefetcher {
    pub fn new(concurrency: usize) -> Self {
        Self { concurrency }
    }

    /// Probe whether readahead(2) is available on this kernel.
    pub fn probe() -> bool {
        use std::os::unix::io::AsRawFd;
        let file = match std::fs::File::open("/dev/null") {
            Ok(f) => f,
            Err(_) => return false,
        };
        #[allow(unsafe_code)]
        let ret = unsafe { libc::readahead(file.as_raw_fd(), 0, 0) };
        // ENOSYS means kernel doesn't support readahead; anything else means it exists.
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            err.raw_os_error() != Some(libc::ENOSYS)
        } else {
            true
        }
    }

    fn do_readahead(
        path: &std::path::Path,
        offset: i64,
        length: i64,
    ) -> Result<(), std::io::Error> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NOATIME)
            .open(path)?;

        let _ = nix::fcntl::posix_fadvise(
            &file,
            offset,
            length,
            PosixFadviseAdvice::POSIX_FADV_SEQUENTIAL,
        );

        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let mut off = offset;
        let end = offset + length;
        while off < end {
            let count = ((end - off) as usize).min(128 * 1024);
            #[allow(unsafe_code)]
            let ret = unsafe { libc::readahead(fd, off, count) };
            if ret < 0 {
                return Err(std::io::Error::last_os_error());
            }
            off += count as i64;
        }

        Ok(())
    }
}

#[async_trait]
impl Prefetcher for ReadaheadPrefetcher {
    async fn execute(&self, plan: &PrefetchPlan, stores: &Stores) -> PrefetchReport {
        execute_concurrent(plan, stores, self.concurrency, |path, offset, length| {
            Self::do_readahead(path, offset, length)
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// MadvisePrefetcher — mmap + madvise(MADV_WILLNEED)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MadvisePrefetcher {
    concurrency: usize,
}

impl MadvisePrefetcher {
    pub fn new(concurrency: usize) -> Self {
        Self { concurrency }
    }

    /// Probe whether madvise(MADV_WILLNEED) is available.
    pub fn probe() -> bool {
        // madvise is available on all POSIX systems; always true on Linux.
        true
    }

    fn do_madvise(
        path: &std::path::Path,
        offset: i64,
        length: i64,
    ) -> Result<(), std::io::Error> {
        use nix::sys::mman;

        if length <= 0 {
            return Ok(());
        }

        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NOATIME)
            .open(path)?;

        let len = NonZeroUsize::new(length as usize)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "zero length"))?;

        #[allow(unsafe_code)]
        let addr = unsafe {
            mman::mmap(
                None,
                len,
                mman::ProtFlags::PROT_READ,
                mman::MapFlags::MAP_PRIVATE,
                &file,
                offset,
            )
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?
        };

        #[allow(unsafe_code)]
        let result = unsafe {
            mman::madvise(addr, length as usize, mman::MmapAdvise::MADV_WILLNEED)
                .map_err(|e| std::io::Error::from_raw_os_error(e as i32))
        };

        #[allow(unsafe_code)]
        unsafe {
            let _ = mman::munmap(addr, length as usize);
        }

        result
    }
}

#[async_trait]
impl Prefetcher for MadvisePrefetcher {
    async fn execute(&self, plan: &PrefetchPlan, stores: &Stores) -> PrefetchReport {
        execute_concurrent(plan, stores, self.concurrency, |path, offset, length| {
            Self::do_madvise(path, offset, length)
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// mincore — determine uncached page ranges
// ---------------------------------------------------------------------------

/// Query the page cache via mincore(2) and return contiguous uncached byte ranges.
/// Falls back to the full range if mincore is unavailable.
fn uncached_ranges(
    path: &std::path::Path,
    offset: i64,
    length: i64,
) -> Vec<(i64, i64)> {
    if length <= 0 {
        return vec![];
    }

    #[allow(unsafe_code)]
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    if page_size == 0 {
        return vec![(offset, length)];
    }

    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NOATIME)
        .open(path)
    {
        Ok(f) => f,
        Err(_) => return vec![(offset, length)],
    };

    // Align offset down and length up to page boundaries for mmap.
    let aligned_offset = (offset as usize) & !(page_size - 1);
    let end = (offset as usize) + (length as usize);
    let aligned_end = (end + page_size - 1) & !(page_size - 1);
    let aligned_length = aligned_end - aligned_offset;

    let Some(map_len) = NonZeroUsize::new(aligned_length) else {
        return vec![];
    };

    use nix::sys::mman;

    #[allow(unsafe_code)]
    let addr = match unsafe {
        mman::mmap(
            None,
            map_len,
            mman::ProtFlags::PROT_READ,
            mman::MapFlags::MAP_PRIVATE,
            &file,
            aligned_offset as i64,
        )
    } {
        Ok(a) => a,
        Err(_) => return vec![(offset, length)],
    };

    let num_pages = aligned_length / page_size;
    let mut vec = vec![0u8; num_pages];

    #[allow(unsafe_code)]
    let mincore_ok = unsafe {
        libc::mincore(
            addr.as_ptr() as *mut libc::c_void,
            aligned_length,
            vec.as_mut_ptr(),
        ) == 0
    };

    #[allow(unsafe_code)]
    unsafe {
        let _ = mman::munmap(addr, aligned_length);
    }

    if !mincore_ok {
        return vec![(offset, length)];
    }

    // Convert the page-level cache bitmap into byte ranges of uncached regions.
    // Only consider pages that overlap the original [offset, offset+length) range.
    let orig_start = offset as usize;
    let orig_end = end;
    let mut ranges = Vec::new();
    let mut run_start: Option<usize> = None;

    for (i, &cached) in vec.iter().enumerate() {
        let page_start = aligned_offset + i * page_size;
        let page_end = page_start + page_size;

        // Skip pages entirely outside the original range.
        if page_end <= orig_start || page_start >= orig_end {
            if let Some(start) = run_start.take() {
                let clamped_start = start.max(orig_start) as i64;
                let clamped_end = page_start.min(orig_end) as i64;
                if clamped_end > clamped_start {
                    ranges.push((clamped_start, clamped_end - clamped_start));
                }
            }
            continue;
        }

        let in_cache = (cached & 1) != 0;
        if !in_cache {
            if run_start.is_none() {
                run_start = Some(page_start);
            }
        } else if let Some(start) = run_start.take() {
            let clamped_start = start.max(orig_start) as i64;
            let clamped_end = page_start.min(orig_end) as i64;
            if clamped_end > clamped_start {
                ranges.push((clamped_start, clamped_end - clamped_start));
            }
        }
    }

    // Flush trailing run.
    if let Some(start) = run_start {
        let clamped_start = start.max(orig_start) as i64;
        let clamped_end = orig_end as i64;
        if clamped_end > clamped_start {
            ranges.push((clamped_start, clamped_end - clamped_start));
        }
    }

    ranges
}

// ---------------------------------------------------------------------------
// Shared concurrent execution helper
// ---------------------------------------------------------------------------

async fn execute_concurrent<F>(
    plan: &PrefetchPlan,
    stores: &Stores,
    concurrency: usize,
    readahead_fn: F,
) -> PrefetchReport
where
    F: Fn(&std::path::Path, i64, i64) -> Result<(), std::io::Error> + Send + Sync + 'static + Clone,
{
    let mut report = PrefetchReport::default();
    let concurrency = concurrency.max(1);

    let tasks: Vec<(crate::domain::MapKey, std::sync::Arc<std::path::Path>, i64, i64)> = plan
        .maps
        .iter()
        .filter_map(|map_id| {
            let map = stores.maps.get(*map_id)?;
            Some((
                map.key(),
                map.path.clone(),
                map.offset as i64,
                map.length as i64,
            ))
        })
        .collect();

    let mut stream = stream::iter(tasks).map(move |(map_key, path, offset, length)| {
        let f = readahead_fn.clone();
        async move {
            let join = tokio::task::spawn_blocking(move || {
                // Use mincore to skip already-cached pages.
                let ranges = uncached_ranges(&path, offset, length);
                if ranges.is_empty() {
                    return Ok(()); // fully cached
                }
                for (range_offset, range_length) in ranges {
                    f(&path, range_offset, range_length)?;
                }
                Ok(())
            })
            .await;
            match join {
                Ok(result) => (map_key, result),
                Err(err) => {
                    let err = std::io::Error::other(err);
                    (map_key, Err(err))
                }
            }
        }
    });

    while let Some((map_key, result)) =
        stream.by_ref().buffer_unordered(concurrency).next().await
    {
        match result {
            Ok(()) => report.num_maps += 1,
            Err(err) => {
                debug!(?map_key, %err, "prefetch failed");
                report.failures.push(map_key);
            }
        }
    }

    report.total_bytes = plan.total_bytes;
    report
}
