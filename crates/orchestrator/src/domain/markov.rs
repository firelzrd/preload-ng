#![forbid(unsafe_code)]

use std::fmt;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MarkovState {
    Neither = 0,
    AOnly = 1,
    BOnly = 2,
    Both = 3,
}

impl MarkovState {
    pub fn from_running(a: bool, b: bool) -> Self {
        match (a, b) {
            (false, false) => MarkovState::Neither,
            (true, false) => MarkovState::AOnly,
            (false, true) => MarkovState::BOnly,
            (true, true) => MarkovState::Both,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Debug for MarkovState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            MarkovState::Neither => "Neither",
            MarkovState::AOnly => "AOnly",
            MarkovState::BOnly => "BOnly",
            MarkovState::Both => "Both",
        };
        f.write_str(name)
    }
}
