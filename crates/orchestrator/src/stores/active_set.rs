#![forbid(unsafe_code)]

use crate::domain::ExeId;
use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Debug, Default)]
pub struct ActiveSet {
    last_seen: FxHashMap<ExeId, u64>,
}

impl ActiveSet {
    pub fn update(&mut self, active_now: impl IntoIterator<Item = ExeId>, now: u64) {
        for exe_id in active_now {
            self.last_seen.insert(exe_id, now);
        }
    }

    pub fn prune(&mut self, now: u64, window: u64) -> FxHashSet<ExeId> {
        let mut removed = FxHashSet::default();
        self.last_seen.retain(|exe_id, last| {
            if now.saturating_sub(*last) > window {
                removed.insert(*exe_id);
                false
            } else {
                true
            }
        });
        removed
    }

    pub fn exes(&self) -> FxHashSet<ExeId> {
        self.last_seen.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use slotmap::SlotMap;
    use rustc_hash::FxHashSet;

    proptest! {
        #[test]
        fn prune_removes_exes_outside_window(
            now in 0u64..10_000,
            window in 0u64..10_000,
            seen_times in prop::collection::vec(0u64..10_000, 0..200),
        ) {
            let mut set = ActiveSet::default();
            let mut ids = SlotMap::<ExeId, ()>::with_key();

            for time in seen_times {
                let id = ids.insert(());
                set.update([id], time);
            }

            let before = set.last_seen.clone();
            let removed = set.prune(now, window);

            let expected_removed: FxHashSet<_> = before
                .iter()
                .filter(|(_, last)| now.saturating_sub(**last) > window)
                .map(|(id, _)| *id)
                .collect();
            let expected_remaining: FxHashSet<_> = before
                .keys()
                .copied()
                .filter(|id| !expected_removed.contains(id))
                .collect();

            let remaining: FxHashSet<_> = set.last_seen.keys().copied().collect();

            prop_assert_eq!(removed, expected_removed);
            prop_assert_eq!(remaining, expected_remaining);
        }
    }
}
