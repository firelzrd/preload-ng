#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MemoryPolicy {
    /// Percentage of total memory (clamped to -100..=100).
    pub memtotal: i32,
    /// Percentage of available memory (clamped to -100..=100).
    pub memavailable: i32,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            memtotal: -5,
            memavailable: 95,
        }
    }
}

impl MemoryPolicy {
    pub fn clamp(self) -> Self {
        Self {
            memtotal: self.memtotal.clamp(-100, 100),
            memavailable: self.memavailable.clamp(-100, 100),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn clamp_limits_values(a in -1000i32..1000, b in -1000i32..1000) {
            let policy = MemoryPolicy { memtotal: a, memavailable: b }.clamp();
            prop_assert!((-100..=100).contains(&policy.memtotal));
            prop_assert!((-100..=100).contains(&policy.memavailable));
        }
    }
}
