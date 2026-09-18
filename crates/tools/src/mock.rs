//! A scripted `Upstream` for tests — advertises a fixed tool list and
//! answers calls from a name → result map, recording every call so tests
//! can assert routing reached the right provider.

use std::sync::Mutex;

use serde_json::{Value, json};

use crate::ToolError;
use crate::upstream::Upstream;

pub struct MockUpstream {
    tools: Vec<Value>,
    /// tool name → the CallTool result to return. Missing names refuse.
    results: std::collections::BTreeMap<String, Value>,
    /// Every (name, args) pair this upstream received.
    pub calls: Mutex<Vec<(String, Value)>>,
}

impl MockUpstream {
    /// Advertise `tool_names` (unprefixed, as a real upstream would).
    pub fn new(tool_names: &[&str]) -> Self {
        Self {
            tools: tool_names
                .iter()
                .map(|name| {
                    json!({
                        "name": name,
                        "description": format!("mock {name}"),
                        "inputSchema": {"type": "object"},
                    })
                })
                .collect(),
            results: std::collections::BTreeMap::new(),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Script one tool's reply (a full CallTool result payload).
    pub fn with_result(mut self, name: &str, result: Value) -> Self {
        self.results.insert(name.to_string(), result);
        self
    }

    /// The advertised tool count — what `ProviderState::Connected` records.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// A plain text reply — the common case.
    pub fn text_result(text: &str) -> Value {
        json!({"content": [{"type": "text", "text": text}]})
    }
}

#[async_trait::async_trait]
impl Upstream for MockUpstream {
    async fn list_tools(&self) -> Result<Vec<Value>, ToolError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, ToolError> {
        self.calls
            .lock()
            .unwrap()
            .push((name.to_string(), arguments));
        match self.results.get(name) {
            Some(result) => Ok(result.clone()),
            None => Err(ToolError::refused(
                "not_found",
                format!("mock upstream has no tool '{name}'"),
            )),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
