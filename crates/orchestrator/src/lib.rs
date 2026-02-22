#![deny(unsafe_code)]

pub mod clock;
pub mod domain;
pub mod engine;
pub mod error;
pub(crate) mod math;
pub mod observation;
pub mod persistence;
pub mod prediction;
pub mod prefetch;
pub mod stores;

pub use engine::{ControlEvent, PreloadEngine, ReloadBundle, Services, TickReport};
pub use observation::{
    AdmissionDecision, AdmissionPolicy, AdmissionPolicyStats, CandidateExe, Completeness,
    DefaultAdmissionPolicy, DefaultModelUpdater, FanotifyWatcher, ModelDelta, ModelUpdater,
    Observation, ObservationEvent, ProcfsScanner, RejectReason, ScanWarning, Scanner,
};
pub use persistence::{NoopRepository, SqliteRepository, StateRepository, StoresSnapshot};
pub use prediction::{MarkovPredictor, Prediction, PredictionSummary, Predictor};
pub use prefetch::{
    GreedyPrefetchPlanner, MadvisePrefetcher, NoopPrefetcher, PosixFadvisePrefetcher,
    PrefetchPlan, PrefetchPlanner, PrefetchReport, Prefetcher, ReadPrefetcher,
    ReadaheadPrefetcher,
};

pub use clock::{Clock, SystemClock};
pub use domain::{Exe, ExeId, ExeKey, MapId, MapKey, MapSegment, MarkovState, MemStat};
pub use stores::Stores;
