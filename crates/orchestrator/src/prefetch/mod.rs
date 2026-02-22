#![deny(unsafe_code)]

mod plan;
mod planner;
mod prefetcher;

pub use plan::{PrefetchPlan, PrefetchReport};
pub use planner::{GreedyPrefetchPlanner, PrefetchPlanner};
pub use prefetcher::{
    MadvisePrefetcher, NoopPrefetcher, PosixFadvisePrefetcher, Prefetcher, ReadPrefetcher,
    ReadaheadPrefetcher,
};
