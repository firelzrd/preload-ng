#![forbid(unsafe_code)]

use crate::domain::{ExeId, MarkovState};
use crate::math::fast_exp_neg;
use crate::stores::EdgeKey;
use half::f16;
use half::slice::HalfFloatSliceExt;
use rustc_hash::{FxHashMap, FxHashSet};

/// SoA (Structure of Arrays) storage for Markov edges.
///
/// Each field vector is indexed by the same position; `key_to_index`
/// maps `EdgeKey → usize` for O(1) lookup.  This layout keeps the
/// f16 probability arrays contiguous in memory, enabling efficient
/// SIMD iteration and better cache utilisation during prediction.
#[derive(Debug, Default)]
pub struct MarkovGraph {
    keys: Vec<EdgeKey>,
    states: Vec<MarkovState>,
    last_change_times: Vec<u64>,
    state_last_left: Vec<[u64; 4]>,
    time_to_leave: Vec<[f16; 4]>,
    transition_prob: Vec<[[f16; 4]; 4]>,
    both_running_times: Vec<u64>,
    key_to_index: FxHashMap<EdgeKey, usize>,
}

/// Read-only view into a single Markov edge stored in SoA layout.
pub struct EdgeRef<'a> {
    pub state: MarkovState,
    pub last_change_time: u64,
    pub state_last_left: &'a [u64; 4],
    pub time_to_leave: &'a [f16; 4],
    pub transition_prob: &'a [[f16; 4]; 4],
    pub both_running_time: u64,
}

impl EdgeRef<'_> {
    /// Batch-convert `time_to_leave` to f32 using F16C when available.
    #[inline]
    pub fn time_to_leave_f32(&self) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        self.time_to_leave.convert_to_f32_slice(&mut out);
        out
    }

    /// Batch-convert `transition_prob` to f32 using F16C when available.
    #[inline]
    pub fn transition_prob_f32(&self) -> [[f32; 4]; 4] {
        let mut out = [[0.0f32; 4]; 4];
        for i in 0..4 {
            self.transition_prob[i].convert_to_f32_slice(&mut out[i]);
        }
        out
    }
}

/// Mutable view into a single Markov edge stored in SoA layout.
pub struct EdgeRefMut<'a> {
    pub state: &'a mut MarkovState,
    pub last_change_time: &'a mut u64,
    pub state_last_left: &'a mut [u64; 4],
    pub time_to_leave: &'a mut [f16; 4],
    pub transition_prob: &'a mut [[f16; 4]; 4],
    pub both_running_time: &'a mut u64,
}

impl EdgeRefMut<'_> {
    /// Batch-convert `time_to_leave` to f32 using F16C when available.
    #[inline]
    pub fn time_to_leave_f32(&self) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        self.time_to_leave.convert_to_f32_slice(&mut out);
        out
    }

    /// Batch-convert `transition_prob` to f32 using F16C when available.
    #[inline]
    pub fn transition_prob_f32(&self) -> [[f32; 4]; 4] {
        let mut out = [[0.0f32; 4]; 4];
        for i in 0..4 {
            self.transition_prob[i].convert_to_f32_slice(&mut out[i]);
        }
        out
    }

    /// Batch-convert f32 values to f16 and store in `time_to_leave`.
    #[inline]
    pub fn set_time_to_leave_f32(&mut self, values: [f32; 4]) {
        self.time_to_leave.convert_from_f32_slice(&values);
    }

    /// Batch-convert f32 values to f16 and store in `transition_prob`.
    #[inline]
    pub fn set_transition_prob_f32(&mut self, values: [[f32; 4]; 4]) {
        for i in 0..4 {
            self.transition_prob[i].convert_from_f32_slice(&values[i]);
        }
    }

    /// Update the edge state and statistics when a transition occurs.
    pub fn update_state(&mut self, new_state: MarkovState, now: u64, decay: f32) {
        if new_state == *self.state {
            return;
        }

        let old_state = *self.state;
        let old_ix = old_state.index();
        let new_ix = new_state.index();

        let dt_since_left = now.saturating_sub(self.state_last_left[old_ix]);
        let dt_since_change = now.saturating_sub(*self.last_change_time);

        let mix_tt = fast_exp_neg(-decay * dt_since_left as f32);
        let mix_tp = fast_exp_neg(-decay * dt_since_change as f32);

        let dwell = dt_since_change as f32;
        let mut ttl_f32 = [0.0f32; 4];
        self.time_to_leave.convert_to_f32_slice(&mut ttl_f32);
        ttl_f32[old_ix] = mix_tt * ttl_f32[old_ix] + (1.0 - mix_tt) * dwell;
        self.time_to_leave.convert_from_f32_slice(&ttl_f32);

        // Process transition_prob rows without diagonal-skip branch to
        // enable autovectorization.  Diagonal values are never read by
        // the prediction path, so computing them is harmless.
        let one_minus_mix = 1.0 - mix_tp;
        for i in 0..4 {
            let has_target = (i == old_ix) as u8 as f32;
            let mut row_f32 = [0.0f32; 4];
            self.transition_prob[i].convert_to_f32_slice(&mut row_f32);
            for j in 0..4 {
                let target = has_target * (j == new_ix) as u8 as f32;
                row_f32[j] = mix_tp * row_f32[j] + one_minus_mix * target;
            }
            self.transition_prob[i].convert_from_f32_slice(&row_f32);
        }

        self.state_last_left[old_ix] = now;
        *self.last_change_time = now;
        *self.state = new_state;
    }
}

impl MarkovGraph {
    pub fn ensure_edge(&mut self, a: ExeId, b: ExeId, now: u64, state: MarkovState) -> bool {
        let key = EdgeKey::new(a, b);
        if self.key_to_index.contains_key(&key) {
            return false;
        }
        let idx = self.keys.len();
        self.keys.push(key);
        self.states.push(state);
        self.last_change_times.push(now);
        self.state_last_left.push([now; 4]);
        self.time_to_leave.push([f16::ZERO; 4]);
        self.transition_prob.push([[f16::ZERO; 4]; 4]);
        self.both_running_times.push(0);
        self.key_to_index.insert(key, idx);
        true
    }

    pub fn get_mut(&mut self, key: EdgeKey) -> Option<EdgeRefMut<'_>> {
        let idx = *self.key_to_index.get(&key)?;
        Some(EdgeRefMut {
            state: &mut self.states[idx],
            last_change_time: &mut self.last_change_times[idx],
            state_last_left: &mut self.state_last_left[idx],
            time_to_leave: &mut self.time_to_leave[idx],
            transition_prob: &mut self.transition_prob[idx],
            both_running_time: &mut self.both_running_times[idx],
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = (EdgeKey, EdgeRef<'_>)> {
        self.keys.iter().enumerate().map(|(i, &key)| {
            (
                key,
                EdgeRef {
                    state: self.states[i],
                    last_change_time: self.last_change_times[i],
                    state_last_left: &self.state_last_left[i],
                    time_to_leave: &self.time_to_leave[i],
                    transition_prob: &self.transition_prob[i],
                    both_running_time: self.both_running_times[i],
                },
            )
        })
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (EdgeKey, EdgeRefMut<'_>)> {
        let MarkovGraph {
            keys,
            states,
            last_change_times,
            state_last_left,
            time_to_leave,
            transition_prob,
            both_running_times,
            key_to_index: _,
        } = self;

        keys.iter()
            .copied()
            .zip(states.iter_mut())
            .zip(last_change_times.iter_mut())
            .zip(state_last_left.iter_mut())
            .zip(time_to_leave.iter_mut())
            .zip(transition_prob.iter_mut())
            .zip(both_running_times.iter_mut())
            .map(|((((((key, state), lct), sll), ttl), tp), brt)| {
                (
                    key,
                    EdgeRefMut {
                        state,
                        last_change_time: lct,
                        state_last_left: sll,
                        time_to_leave: ttl,
                        transition_prob: tp,
                        both_running_time: brt,
                    },
                )
            })
    }

    pub fn prune_inactive(&mut self, active: &FxHashSet<ExeId>) {
        let mut i = 0;
        while i < self.keys.len() {
            let key = self.keys[i];
            if active.contains(&key.0) && active.contains(&key.1) {
                i += 1;
            } else {
                self.swap_remove(i);
            }
        }
    }

    fn swap_remove(&mut self, idx: usize) {
        let last = self.keys.len() - 1;
        if idx != last {
            let moved_key = self.keys[last];
            self.key_to_index.insert(moved_key, idx);
        }
        let removed_key = self.keys[idx];
        self.key_to_index.remove(&removed_key);

        self.keys.swap_remove(idx);
        self.states.swap_remove(idx);
        self.last_change_times.swap_remove(idx);
        self.state_last_left.swap_remove(idx);
        self.time_to_leave.swap_remove(idx);
        self.transition_prob.swap_remove(idx);
        self.both_running_times.swap_remove(idx);
    }
}
