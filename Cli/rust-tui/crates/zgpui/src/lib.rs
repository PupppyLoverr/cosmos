//! Typed ACP client for Cosmos GPUI. zcli itself does not link GPUI.
//! Transport: spawn `zcli acp` and speak JSON-RPC over stdio.

pub mod client;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUpdate {
    pub session_id: String,
    pub kind: String,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub session_id: String,
    pub tool: String,
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub usd: Option<f64>,
    pub cum_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GpuiEvent {
    SessionUpdate(SessionUpdate),
    Permission(PermissionRequest),
    Usage(Usage),
    Compact { before: u64, after: u64 },
    SelfImprove { surface: String },
    Pong { text: String },
}
