//! meridian_bridge — shared memory/context bridge for the Cosmos CLI family.
//!
//! One Rust implementation. zcli links this crate; zdev and zagy talk to
//! `meridian serve --stdio` (newline-delimited JSON-RPC 2.0) through thin
//! clients. The vault is markdown files under `MERIDIAN_VAULT`
//! (default `%USERPROFILE%\.meridian\vault`).

pub mod boot;
pub mod memory;
pub mod rpc;
pub mod secrets;
pub mod search;
pub mod summarize;
pub mod tokens;
pub mod vault;

pub use vault::Vault;
