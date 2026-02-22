#![forbid(unsafe_code)]

use crate::domain::{MapSegment, MemStat};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum ObservationEvent {
    ObsBegin {
        time: u64,
        scan_id: u64,
    },
    ExeSeen {
        path: Arc<Path>,
        pid: u32,
    },
    MapSeen {
        exe_path: Arc<Path>,
        map: MapSegment,
    },
    MemStat {
        mem: MemStat,
    },
    ObsEnd {
        time: u64,
        scan_id: u64,
        warnings: Vec<ScanWarning>,
    },
}

pub type Observation = Vec<ObservationEvent>;

#[derive(Debug, Clone)]
pub enum ScanWarning {
    MapScanFailed { pid: u32, reason: String },
}
