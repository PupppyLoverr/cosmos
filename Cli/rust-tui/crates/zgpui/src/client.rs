//! Spawn `zcli acp`, speak JSON-RPC, yield typed events.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};

use crate::GpuiEvent;

pub struct AcpClient {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: i64,
}

impl AcpClient {
    pub fn spawn(zcli: PathBuf) -> std::io::Result<Self> {
        let mut child = Command::new(zcli)
            .arg("acp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        })
    }

    pub fn rpc(&mut self, method: &str, params: Value) -> std::io::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        writeln!(self.stdin, "{req}")?;
        self.stdin.flush()?;
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line)? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "acp closed",
                ));
            }
            let v: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
            if v.get("id") == Some(&json!(id)) {
                if let Some(err) = v.get("error") {
                    return Err(std::io::Error::other(err.to_string()));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            // notifications ignored here
        }
    }

    pub fn initialize(&mut self) -> std::io::Result<Value> {
        self.rpc("initialize", json!({"protocolVersion": 1}))
    }

    pub fn session_new(&mut self, cwd: &str) -> std::io::Result<String> {
        let r = self.rpc("session/new", json!({"cwd": cwd}))?;
        Ok(r["sessionId"].as_str().unwrap_or("").to_string())
    }

    pub fn prompt(&mut self, session_id: &str, text: &str) -> std::io::Result<Vec<GpuiEvent>> {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({
            "jsonrpc":"2.0","id":id,"method":"session/prompt",
            "params":{"sessionId":session_id,"prompt":[{"type":"text","text":text}]}
        });
        writeln!(self.stdin, "{req}")?;
        self.stdin.flush()?;
        let mut evs = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line)? == 0 {
                break;
            }
            let v: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
            if v.get("id") == Some(&json!(id)) {
                break;
            }
            if v["method"] == "session/update" {
                let text = v["params"]["update"]["content"]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                if text.contains("pong") {
                    evs.push(GpuiEvent::Pong { text });
                } else {
                    evs.push(GpuiEvent::SessionUpdate(crate::SessionUpdate {
                        session_id: session_id.into(),
                        kind: "update".into(),
                        data: v["params"].clone(),
                    }));
                }
            }
        }
        Ok(evs)
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
