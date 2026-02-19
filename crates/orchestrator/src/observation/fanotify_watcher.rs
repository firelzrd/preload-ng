#![forbid(unsafe_code)]

use crate::domain::MapSegment;
use crate::observation::ObservationEvent;
use nix::sys::fanotify::{EventFFlags, Fanotify, InitFlags, MarkFlags, MaskFlags};
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tracing::{info, trace, warn};

const SKIP_PREFIXES: &[&str] = &[
    "/proc/",
    "/sys/",
    "/dev/",
    "/tmp/",
    "/run/",
    "/var/run/",
    "/var/lock/",
];

#[derive(Default)]
struct EventBuffer {
    maps: HashMap<(PathBuf, PathBuf), u64>,
    exes: HashMap<PathBuf, u32>,
}

pub struct FanotifyWatcher {
    buffer: Arc<Mutex<EventBuffer>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FanotifyWatcher {
    pub fn try_new() -> Option<Arc<Self>> {
        let fan = match Fanotify::init(
            InitFlags::FAN_CLOEXEC | InitFlags::FAN_CLASS_NOTIF | InitFlags::FAN_NONBLOCK,
            EventFFlags::O_RDONLY | EventFFlags::O_CLOEXEC | EventFFlags::O_LARGEFILE,
        ) {
            Ok(f) => f,
            Err(err) => {
                warn!(?err, "fanotify init failed (need CAP_SYS_ADMIN)");
                return None;
            }
        };

        let root = match std::fs::File::open("/") {
            Ok(f) => f,
            Err(err) => {
                warn!(?err, "failed to open / for fanotify mark");
                return None;
            }
        };

        if let Err(err) = fan.mark(
            MarkFlags::FAN_MARK_ADD | MarkFlags::FAN_MARK_FILESYSTEM,
            MaskFlags::FAN_OPEN,
            &root,
            None::<&std::path::Path>,
        ) {
            warn!(?err, "fanotify mark failed");
            return None;
        }

        let buffer = Arc::new(Mutex::new(EventBuffer::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let handle = {
            let buffer = Arc::clone(&buffer);
            let stop = Arc::clone(&stop);
            match std::thread::Builder::new()
                .name("fanotify-reader".into())
                .spawn(move || Self::reader_loop(fan, buffer, stop))
            {
                Ok(h) => h,
                Err(err) => {
                    warn!(?err, "failed to spawn fanotify reader thread");
                    return None;
                }
            }
        };

        info!("fanotify watcher started");
        Some(Arc::new(Self {
            buffer,
            stop,
            handle: Some(handle),
        }))
    }

    fn reader_loop(
        fan: Fanotify,
        buffer: Arc<Mutex<EventBuffer>>,
        stop: Arc<AtomicBool>,
    ) {
        let self_pid = std::process::id() as i32;

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }

            let events = match fan.read_events() {
                Ok(events) => events,
                Err(nix::errno::Errno::EAGAIN) => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
                Err(nix::errno::Errno::EINTR) => continue,
                Err(err) => {
                    warn!(?err, "fanotify read_events failed");
                    break;
                }
            };

            for event in &events {
                let Some(fd) = event.fd() else {
                    continue; // queue overflow
                };

                let pid = event.pid();
                if pid == self_pid || pid <= 0 {
                    continue;
                }

                let raw_fd = fd.as_raw_fd();

                // Resolve file path from fd.
                let file_path = match std::fs::read_link(format!("/proc/self/fd/{raw_fd}")) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Skip virtual/temp filesystems.
                let path_str = match file_path.to_str() {
                    Some(s) => s,
                    None => continue,
                };
                if SKIP_PREFIXES.iter().any(|prefix| path_str.starts_with(prefix)) {
                    continue;
                }

                // Only regular files with nonzero size.
                let meta = match std::fs::metadata(&file_path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if !meta.is_file() || meta.len() == 0 {
                    continue;
                }
                let file_size = meta.len();

                // Resolve exe path of the opening process.
                let exe_path = match std::fs::read_link(format!("/proc/{pid}/exe")) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let mut buf = match buffer.lock() {
                    Ok(b) => b,
                    Err(poisoned) => poisoned.into_inner(),
                };
                buf.exes.entry(exe_path.clone()).or_insert(pid as u32);
                buf.maps.entry((exe_path, file_path)).or_insert(file_size);
            }
        }

        trace!("fanotify reader loop exited");
    }

    pub fn drain(&self, time: u64) -> Vec<ObservationEvent> {
        let buf = {
            let mut guard = match self.buffer.lock() {
                Ok(b) => b,
                Err(poisoned) => poisoned.into_inner(),
            };
            std::mem::take(&mut *guard)
        };

        let mut events = Vec::with_capacity(buf.exes.len() + buf.maps.len());

        for (path, pid) in buf.exes {
            events.push(ObservationEvent::ExeSeen { path, pid });
        }

        for ((exe_path, file_path), file_size) in buf.maps {
            events.push(ObservationEvent::MapSeen {
                exe_path,
                map: MapSegment::new(file_path, 0, file_size, time),
            });
        }

        events
    }
}

impl std::fmt::Debug for FanotifyWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (exes, maps) = self
            .buffer
            .lock()
            .map(|b| (b.exes.len(), b.maps.len()))
            .unwrap_or((0, 0));
        f.debug_struct("FanotifyWatcher")
            .field("buffered_exes", &exes)
            .field("buffered_maps", &maps)
            .field("active", &!self.stop.load(Ordering::Relaxed))
            .finish()
    }
}

impl Drop for FanotifyWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
