use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: u8,
    pub node_id: String,
    pub boot_id: String,
    pub uptime_ms: u64,
    pub active_sessions: usize,
    pub forwarded_bytes: u64,
}
