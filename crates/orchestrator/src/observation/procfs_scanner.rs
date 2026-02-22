#![forbid(unsafe_code)]

use crate::domain::{MapSegment, MemStat};
use crate::error::Error;
use crate::observation::fanotify_watcher::FanotifyWatcher;
use crate::observation::{Observation, ObservationEvent, ScanWarning, Scanner};
use procfs::process::MMapPath;
use procfs::{Current, Meminfo, page_size, vmstat};
use rustc_hash::FxHashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, trace, warn};

/// How often (in scan cycles) to re-scan maps of already-known processes.
const DEFAULT_MAP_RESCAN_INTERVAL: u64 = 5;

#[derive(Debug, Clone)]
struct CachedProcess {
    starttime: u64,
    exe_path: Arc<Path>,
    maps: Vec<CachedMap>,
    last_map_scan: u64,
}

#[derive(Debug, Clone)]
struct CachedMap {
    path: Arc<Path>,
    offset: u64,
    length: u64,
    device: u64,
    inode: u64,
}

#[derive(Debug)]
pub struct ProcfsScanner {
    fanotify: Option<Arc<FanotifyWatcher>>,
    cache: FxHashMap<u32, CachedProcess>,
    scan_count: u64,
    map_rescan_interval: u64,
}

impl ProcfsScanner {
    pub fn new(fanotify: Option<Arc<FanotifyWatcher>>) -> Self {
        Self {
            fanotify,
            cache: FxHashMap::default(),
            scan_count: 0,
            map_rescan_interval: DEFAULT_MAP_RESCAN_INTERVAL,
        }
    }
}

impl Default for ProcfsScanner {
    fn default() -> Self {
        Self {
            fanotify: None,
            cache: FxHashMap::default(),
            scan_count: 0,
            map_rescan_interval: DEFAULT_MAP_RESCAN_INTERVAL,
        }
    }
}

impl ProcfsScanner {
    fn sanitize_path(path: &Path) -> Option<Arc<Path>> {
        if !path.has_root() {
            return None;
        }
        let path_str = path.to_str()?;
        if path_str.contains("(deleted)") {
            return None;
        }
        let trimmed = path_str.split(".#prelink#.").next()?;
        Some(Arc::from(Path::new(trimmed)))
    }

    fn read_memstat() -> Result<MemStat, Error> {
        let mem = Meminfo::current()?;
        let vm = vmstat()?;
        let page = page_size() as i64;
        let pagein = vm.get("pgpgin").map(|v| v * page / 1024).unwrap_or(0);
        let pageout = vm.get("pgpgout").map(|v| v * page / 1024).unwrap_or(0);

        Ok(MemStat {
            total: mem.mem_total / 1024,
            available: mem.mem_available.unwrap_or(mem.mem_free + mem.cached) / 1024,
            free: mem.mem_free / 1024,
            cached: mem.cached / 1024,
            pagein,
            pageout,
        })
    }

    /// Read maps for a process and produce events, caching the results.
    fn scan_maps(
        process: &procfs::process::Process,
        exe_path: &Arc<Path>,
        time: u64,
        events: &mut Vec<ObservationEvent>,
        warnings: &mut Vec<ScanWarning>,
    ) -> Vec<CachedMap> {
        let mut cached_maps = Vec::new();
        let pid = process.pid as u32;

        match process.maps() {
            Ok(maps) => {
                for map in maps {
                    let MMapPath::Path(path) = map.pathname else {
                        continue;
                    };
                    let Some(path) = Self::sanitize_path(&path) else {
                        continue;
                    };
                    let (start, end) = map.address;
                    let length = end.saturating_sub(start);
                    let device = ((map.dev.0 as u64) << 20) | (map.dev.1 as u64);
                    let inode = map.inode;
                    cached_maps.push(CachedMap {
                        path: path.clone(),
                        offset: map.offset,
                        length,
                        device,
                        inode,
                    });
                    let mut segment = MapSegment::from_arc(path, map.offset, length, time);
                    segment.device = device;
                    segment.inode = inode;
                    events.push(ObservationEvent::MapSeen {
                        exe_path: exe_path.clone(),
                        map: segment,
                    });
                }
            }
            Err(err) => {
                warnings.push(ScanWarning::MapScanFailed {
                    pid,
                    reason: err.to_string(),
                });
            }
        }

        cached_maps
    }

    /// Emit cached map events without re-reading /proc/PID/maps.
    fn emit_cached_maps(
        exe_path: &Arc<Path>,
        cached_maps: &[CachedMap],
        time: u64,
        events: &mut Vec<ObservationEvent>,
    ) {
        for cm in cached_maps {
            let mut segment = MapSegment::from_arc(cm.path.clone(), cm.offset, cm.length, time);
            segment.device = cm.device;
            segment.inode = cm.inode;
            events.push(ObservationEvent::MapSeen {
                exe_path: exe_path.clone(),
                map: segment,
            });
        }
    }
}

impl Scanner for ProcfsScanner {
    fn scan(&mut self, time: u64, scan_id: u64) -> Result<Observation, Error> {
        self.scan_count += 1;
        let mut events = Vec::new();
        let mut warnings = Vec::new();
        events.push(ObservationEvent::ObsBegin { time, scan_id });

        // Track which PIDs are seen this cycle.
        let mut seen_pids =
            FxHashMap::with_capacity_and_hasher(self.cache.len(), Default::default());

        for process in procfs::process::all_processes()? {
            let process = match process {
                Ok(p) => p,
                Err(err) => {
                    warn!(?err, "failed to read process entry");
                    continue;
                }
            };
            let pid = process.pid as u32;

            // Get starttime for PID reuse detection.
            let starttime = match process.stat() {
                Ok(stat) => stat.starttime,
                Err(_) => continue,
            };

            // Check cache for this PID.
            let cached = self.cache.get(&pid);
            let is_same_process = cached
                .map(|c| c.starttime == starttime)
                .unwrap_or(false);

            if is_same_process {
                let cached = cached.unwrap();
                let exe_path = cached.exe_path.clone();
                events.push(ObservationEvent::ExeSeen {
                    path: exe_path.clone(),
                    pid,
                });

                // Rescan maps periodically.
                let cycles_since = self.scan_count.saturating_sub(cached.last_map_scan);
                if cycles_since >= self.map_rescan_interval {
                    let maps = Self::scan_maps(&process, &exe_path, time, &mut events, &mut warnings);
                    seen_pids.insert(pid, CachedProcess {
                        starttime,
                        exe_path,
                        maps,
                        last_map_scan: self.scan_count,
                    });
                } else {
                    Self::emit_cached_maps(&exe_path, &cached.maps, time, &mut events);
                    seen_pids.insert(pid, cached.clone());
                }
            } else {
                // New PID or PID reuse: full scan.
                let exe_path = match process.exe() {
                    Ok(path) => path,
                    Err(err) => {
                        warn!(pid, ?err, "failed to read exe path");
                        continue;
                    }
                };
                let Some(exe_path) = Self::sanitize_path(&exe_path) else {
                    continue;
                };

                events.push(ObservationEvent::ExeSeen {
                    path: exe_path.clone(),
                    pid,
                });

                let maps = Self::scan_maps(&process, &exe_path, time, &mut events, &mut warnings);
                seen_pids.insert(pid, CachedProcess {
                    starttime,
                    exe_path,
                    maps,
                    last_map_scan: self.scan_count,
                });
            }
        }

        // Replace cache with current PIDs only (prunes dead PIDs).
        self.cache = seen_pids;

        // Drain fanotify events (file-open monitoring).
        if let Some(watcher) = &self.fanotify {
            let fan_events = watcher.drain(time);
            let fan_exes = fan_events.iter().filter(|e| matches!(e, ObservationEvent::ExeSeen { .. })).count();
            let fan_maps = fan_events.iter().filter(|e| matches!(e, ObservationEvent::MapSeen { .. })).count();
            info!(fan_exes, fan_maps, "fanotify drain");
            events.extend(fan_events);
        }

        if let Ok(mem) = Self::read_memstat() {
            events.push(ObservationEvent::MemStat { mem });
        }

        events.push(ObservationEvent::ObsEnd {
            time,
            scan_id,
            warnings,
        });

        trace!(scan_id, event_count = events.len(), cached_pids = self.cache.len(), "observation collected");
        Ok(events)
    }
}
