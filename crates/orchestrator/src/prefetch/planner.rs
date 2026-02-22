#![forbid(unsafe_code)]

use crate::domain::{MapId, MemStat};
use crate::prediction::Prediction;
use crate::prefetch::PrefetchPlan;
use crate::stores::Stores;
use config::{Config, SortStrategy};
use half::f16;
use std::cmp::Ordering;
use tracing::trace;

/// Default block size for computing block index when metadata is unavailable.
const DEFAULT_BLOCK_SIZE: u64 = 4096;

pub trait PrefetchPlanner: Send + Sync {
    /// Create a prefetch plan from prediction scores and memory stats.
    fn plan(&self, prediction: &Prediction, stores: &Stores, memstat: &MemStat) -> PrefetchPlan;
}

#[derive(Debug)]
pub struct GreedyPrefetchPlanner {
    sort: SortStrategy,
    memtotal: i32,
    memavailable: i32,
}

impl GreedyPrefetchPlanner {
    pub fn new(config: &Config) -> Self {
        let policy = config.model.memory.clamp();
        Self {
            sort: config.system.sortstrategy,
            memtotal: policy.memtotal,
            memavailable: policy.memavailable,
        }
    }

    fn available_kb(&self, mem: &MemStat) -> u64 {
        let mut budget = self.memtotal as i64 * mem.total as i64 / 100;
        budget += self.memavailable as i64 * mem.available as i64 / 100;
        budget.max(0) as u64
    }

    fn kb(bytes: u64) -> u64 {
        bytes.div_ceil(1024)
    }
}

impl PrefetchPlanner for GreedyPrefetchPlanner {
    fn plan(&self, prediction: &Prediction, stores: &Stores, memstat: &MemStat) -> PrefetchPlan {
        let mut items: Vec<(MapId, f16)> = prediction
            .map_scores
            .iter()
            .filter(|(_, score)| **score > f16::ZERO)
            .map(|(id, score)| (*id, *score))
            .collect();
        items.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));

        let mut budget_kb = self.available_kb(memstat);
        let mut selected = Vec::new();
        let mut total_bytes: u64 = 0;

        for (map_id, score) in items {
            let Some(map) = stores.maps.get(map_id) else {
                continue;
            };
            let map_kb = Self::kb(map.length);
            if map_kb > budget_kb {
                continue;
            }
            budget_kb = budget_kb.saturating_sub(map_kb);
            total_bytes = total_bytes.saturating_add(map.length);
            selected.push(SelectedMap {
                id: map_id,
                score,
                index: selected.len(),
            });
        }

        // Sort selected maps based on strategy for I/O efficiency.
        match self.sort {
            SortStrategy::None => {}
            SortStrategy::Path => {
                let mut keyed: Vec<SelectedWithKey<std::sync::Arc<std::path::Path>>> = selected
                    .into_iter()
                    .map(|item| {
                        let key = stores.maps.get(item.id).map(|m| m.path.clone());
                        SelectedWithKey { item, key }
                    })
                    .collect();
                sort_by_score_and_key(&mut keyed);
                selected = keyed.into_iter().map(|entry| entry.item).collect();
            }
            SortStrategy::Block => {
                let mut keyed: Vec<SelectedWithKey<BlockKey>> = selected
                    .into_iter()
                    .map(|item| {
                        let key = stores.maps.get(item.id).and_then(|map| {
                            if map.device == 0 && map.inode == 0 {
                                return None;
                            }
                            let block = map.offset / DEFAULT_BLOCK_SIZE;
                            Some(BlockKey {
                                device: map.device,
                                block,
                                offset: map.offset,
                            })
                        });
                        SelectedWithKey { item, key }
                    })
                    .collect();
                sort_by_score_and_key(&mut keyed);
                selected = keyed.into_iter().map(|entry| entry.item).collect();
            }
            SortStrategy::Inode => {
                let mut keyed: Vec<SelectedWithKey<InodeKey>> = selected
                    .into_iter()
                    .map(|item| {
                        let key = stores.maps.get(item.id).and_then(|map| {
                            if map.device == 0 && map.inode == 0 {
                                return None;
                            }
                            Some(InodeKey {
                                device: map.device,
                                inode: map.inode,
                                offset: map.offset,
                            })
                        });
                        SelectedWithKey { item, key }
                    })
                    .collect();
                sort_by_score_and_key(&mut keyed);
                selected = keyed.into_iter().map(|entry| entry.item).collect();
            }
        }

        trace!(
            selected = selected.len(),
            total_bytes, "prefetch plan created"
        );

        PrefetchPlan {
            maps: selected.into_iter().map(|item| item.id).collect(),
            total_bytes,
            budget_bytes: self.available_kb(memstat) * 1024,
        }
    }
}

#[derive(Debug, Clone)]
struct SelectedMap {
    id: MapId,
    score: f16,
    index: usize,
}

#[derive(Debug, Clone)]
struct SelectedWithKey<K> {
    item: SelectedMap,
    key: Option<K>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BlockKey {
    device: u64,
    block: u64,
    offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct InodeKey {
    device: u64,
    inode: u64,
    offset: u64,
}

fn sort_by_score_and_key<K: Ord>(items: &mut [SelectedWithKey<K>]) {
    items.sort_unstable_by(|a, b| {
        let score_cmp = b.item.score.total_cmp(&a.item.score);
        if score_cmp != Ordering::Equal {
            return score_cmp;
        }
        match (&a.key, &b.key) {
            (Some(a_key), Some(b_key)) => a_key
                .cmp(b_key)
                .then_with(|| a.item.index.cmp(&b.item.index)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.item.index.cmp(&b.item.index),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MapSegment;
    use crate::prediction::Prediction;
    use crate::stores::Stores;
    use config::{Config, MemoryPolicy, SortStrategy};
    use proptest::prelude::*;
    use std::collections::HashSet;

    proptest! {
        #[test]
        fn planner_respects_budget_and_uniqueness(
            maps in prop::collection::vec((1u64..8192, 0f32..1f32), 0..20),
            memtotal in -100i32..100,
            memavailable in -100i32..100,
            total in 0u64..1024,
            available in 0u64..1024,
        ) {
            let mut config = Config::default();
            config.model.memory = MemoryPolicy { memtotal, memavailable };
            config.system.sortstrategy = SortStrategy::None;

            let planner = GreedyPrefetchPlanner::new(&config);
            let mut stores = Stores::default();
            let mut prediction = Prediction::default();

            for (idx, (size, score)) in maps.iter().enumerate() {
                let map_id = stores.ensure_map(MapSegment::new(
                    format!("/map/{idx}"),
                    0,
                    *size,
                    0,
                ));
                prediction.map_scores.insert(map_id, f16::from_f32(*score));
            }

            let mem = MemStat {
                total,
                available,
                free: 0,
                cached: 0,
                pagein: 0,
                pageout: 0,
            };

            let plan = planner.plan(&prediction, &stores, &mem);
            let budget_bytes = planner.available_kb(&mem) * 1024;

            prop_assert!(plan.total_bytes <= budget_bytes);

            let unique: HashSet<_> = plan.maps.iter().copied().collect();
            prop_assert_eq!(unique.len(), plan.maps.len());

            if budget_bytes == 0 {
                prop_assert!(plan.maps.is_empty());
                prop_assert_eq!(plan.total_bytes, 0);
            }
        }
    }
}
