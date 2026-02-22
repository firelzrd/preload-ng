#![forbid(unsafe_code)]

use crate::domain::{ExeId, MapId};
use half::f16;
use rustc_hash::FxHashMap;

#[derive(Debug, Default, Clone)]
pub struct Prediction {
    pub exe_scores: FxHashMap<ExeId, f16>,
    pub map_scores: FxHashMap<MapId, f16>,
}

#[derive(Debug, Default, Clone)]
pub struct PredictionSummary {
    pub num_exes_scored: usize,
    pub num_maps_scored: usize,
}

impl Prediction {
    pub fn summarize(&self) -> PredictionSummary {
        PredictionSummary {
            num_exes_scored: self.exe_scores.len(),
            num_maps_scored: self.map_scores.len(),
        }
    }
}
