//! Read-only copy of the live state, served off the pump path.

use std::collections::HashMap;

use crate::world::{ChatLine, Modal};

/// Heartbeat snapshot: plain data, no connection attached.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub position: (i32, i32, i32),
    pub ticks: u64,
    pub modal: Option<Modal>,
    pub stats: HashMap<u8, (u8, i32)>,
    pub inv: HashMap<u16, HashMap<u8, (u16, u32)>>,
    pub inv_cap: HashMap<u16, u8>,
    /// Canonical backpack com (`inventory:inv`) resolved from the component
    /// table, so readers never pick a stale shop/bank panel by capacity.
    pub backpack_com: Option<u16>,
    pub texts: HashMap<u16, String>,
    /// Last received private tells, oldest first (capped: tells are rare
    /// but the log is unbounded). Makes reception verifiable off the
    /// pump path, like chat and messages.
    pub private: Vec<ChatLine>,
    pub energy: u8,
    pub intent: String,
}
