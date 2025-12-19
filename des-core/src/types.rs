//! Core type definitions and newtypes for the simulation framework

use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for events in the simulation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(pub u64);

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Event({})", self.0)
    }
}





