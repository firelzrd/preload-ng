#![deny(unsafe_code)]

//! Process scheduling priority helpers.
//!
//! Sets the daemon to minimum CPU and I/O priority so prefetching yields
//! to all other work on the system.

use tracing::{info, warn};

const IOPRIO_WHO_PROCESS: i32 = 1;
const IOPRIO_CLASS_IDLE: i32 = 3;
const IOPRIO_CLASS_SHIFT: i32 = 13;

/// Lower the process CPU and I/O scheduling priority.
///
/// Both calls are best-effort: failures are logged as warnings but do not
/// prevent the daemon from running.
pub fn lower_process_priority() {
    set_nice(19);
    set_ionice_idle();
}

/// Increase the nice value of the calling process.
///
/// `nice()` can legitimately return −1 as a new nice value, so errors are
/// detected via errno rather than the return value (POSIX convention).
fn set_nice(inc: i32) {
    #[allow(unsafe_code)]
    unsafe {
        *libc::__errno_location() = 0;
    }

    #[allow(unsafe_code)]
    let ret = unsafe { libc::nice(inc) };

    #[allow(unsafe_code)]
    let errno = unsafe { *libc::__errno_location() };

    if errno != 0 {
        let err = std::io::Error::from_raw_os_error(errno);
        warn!(%err, inc, "failed to set nice value");
    } else {
        info!(nice = ret, "process nice value set");
    }
}

/// Set the I/O scheduling class to IDLE (class 3, priority 0).
///
/// IDLE means prefetch I/O only runs when no other process needs disk,
/// eliminating contention with foreground applications.
fn set_ionice_idle() {
    let ioprio = (IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT) | 0;

    #[allow(unsafe_code)]
    let ret = unsafe {
        libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, 0, ioprio)
    };

    if ret < 0 {
        let err = std::io::Error::last_os_error();
        warn!(%err, "failed to set I/O priority to IDLE class");
    } else {
        info!("I/O scheduling class set to IDLE");
    }
}
