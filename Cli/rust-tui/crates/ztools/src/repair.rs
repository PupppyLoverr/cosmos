//! Tool-call repair. Open models emit broken JSON / wrong types / missing
//! fields / alias names. Repair locally, log `tool_repair`, continue.
//!
//! Port this file's algorithm comments to zdev `repair.ts` and zagy `repair.go`.
//!
//! Algorithm (mandatory order):
//! 1. Parse. If JSON is truncated or wrapped in markdown fences, unwrap and
//!    parse again.
//! 2. If `name` is a close alias (`Read`/`file_read`/`read_file` → `read`), remap.
//! 3. Fill missing optional fields with defaults. If a required field is
//!    missing, do not execute; emit a 1-line tool_result:
//!    `repair: missing field path — resend`.
//! 4. Coerce obvious types (`start: "12"` → 12).
//! 5. Cap unknown keys (`additionalProperties: false`).
//! 6. Emit `{from, to, reason}` so we can count repairs.

use serde_json::{json, Map, Value};

use crate::TOOLS_JSON;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    pub from: String,
    pub to: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct RepairResult {
    pub name: String,
    pub input: Value,
    pub repairs: Vec<Repair>,
    /// When set, do not execute — return this as the tool_result body.
    pub reject: Option<String>,
}

fn aliases() -> &'static [(&'static str, &'static str)] {
    &[
        ("Read", "read"),
        ("read_file", "read"),
        ("file_read", "read"),
        ("Write", "write"),
        ("write_file", "write"),
        ("file_write", "write"),
        ("Edit", "edit"),
        ("StrReplace", "edit"),
        ("str_replace", "edit"),
        ("Bash", "bash"),
        ("Shell", "bash"),
        ("shell", "bash"),
        ("Grep", "grep"),
        ("Glob", "glob"),
        ("Ls", "ls"),
        ("List", "ls"),
        ("MemorySearch", "memory_search"),
        ("memory.search", "memory_search"),
        ("MemoryWrite", "memory_write"),
        ("Task", "task"),
        ("TodoWrite", "task"),
    ]
}

pub fn remap_name(name: &str) -> (String, Option<Repair>) {
    let trimmed = name.trim();
    for (from, to) in aliases() {
        if trimmed == *from || trimmed.eq_ignore_ascii_case(from) && *from != *to {
            if trimmed != *to {
                return (
                    (*to).to_string(),
                    Some(Repair {
                        from: trimmed.to_string(),
                        to: (*to).to_string(),
                        reason: "alias".into(),
                    }),
                );
            }
        }
    }
    // case-insensitive match against canonical names
    if let Ok(specs) = serde_json::from_str::<Vec<Value>>(TOOLS_JSON) {
        for s in specs {
            if let Some(n) = s.get("name").and_then(|v| v.as_str()) {
                if trimmed.eq_ignore_ascii_case(n) && trimmed != n {
                    return (
                        n.to_string(),
                        Some(Repair {
                            from: trimmed.to_string(),
                            to: n.to_string(),
                            reason: "case".into(),
                        }),
                    );
                }
            }
        }
    }
    (trimmed.to_string(), None)
}

/// Unwrap ```json ... ``` or `{...}` trailing junk.
pub fn unwrap_json(raw: &str) -> String {
    let s = raw.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s).trim();
    if let Some(start) = s.find('{') {
        if let Some(end) = s.rfind('}') {
            if end > start {
                return s[start..=end].to_string();
            }
        }
        // truncated object — close it
        let mut body = s[start..].to_string();
        let opens = body.chars().filter(|c| *c == '{').count();
        let closes = body.chars().filter(|c| *c == '}').count();
        for _ in 0..opens.saturating_sub(closes) {
            body.push('}');
        }
        return body;
    }
    s.to_string()
}

fn spec_for(name: &str) -> Option<Value> {
    let specs: Vec<Value> = serde_json::from_str(TOOLS_JSON).ok()?;
    specs.into_iter().find(|s| s.get("name").and_then(|v| v.as_str()) == Some(name))
}

fn required_fields(spec: &Value) -> Vec<String> {
    spec.pointer("/input_schema/required")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn known_keys(spec: &Value) -> Vec<String> {
    spec.pointer("/input_schema/properties")
        .and_then(|v| v.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

fn prop_type(spec: &Value, key: &str) -> Option<String> {
    spec.pointer(&format!("/input_schema/properties/{key}/type"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn coerce(v: &Value, ty: &str) -> Option<Value> {
    match ty {
        "integer" => {
            if v.is_i64() || v.is_u64() {
                return Some(v.clone());
            }
            if let Some(s) = v.as_str() {
                if let Ok(n) = s.trim().parse::<i64>() {
                    return Some(json!(n));
                }
            }
            if let Some(f) = v.as_f64() {
                return Some(json!(f as i64));
            }
            None
        }
        "boolean" => {
            if v.is_boolean() {
                return Some(v.clone());
            }
            if let Some(s) = v.as_str() {
                return Some(json!(s == "true" || s == "1" || s == "yes"));
            }
            None
        }
        "string" => Some(json!(match v {
            Value::String(s) => s.clone(),
            other => other.to_string().trim_matches('"').to_string(),
        })),
        "object" => {
            if v.is_object() {
                Some(v.clone())
            } else {
                None
            }
        }
        "array" => {
            if v.is_array() {
                Some(v.clone())
            } else {
                None
            }
        }
        _ => Some(v.clone()),
    }
}

pub fn repair(name: &str, input: &Value) -> RepairResult {
    let mut repairs = Vec::new();
    let (name, alias) = remap_name(name);
    if let Some(r) = alias {
        repairs.push(r);
    }

    let mut obj: Map<String, Value> = match input {
        Value::Object(m) => m.clone(),
        Value::String(s) => match serde_json::from_str(&unwrap_json(s)) {
            Ok(Value::Object(m)) => {
                repairs.push(Repair {
                    from: "string".into(),
                    to: "object".into(),
                    reason: "unwrap".into(),
                });
                m
            }
            _ => Map::new(),
        },
        _ => Map::new(),
    };

    let spec = spec_for(&name);
    if let Some(spec) = &spec {
        let known = known_keys(spec);
        let keys: Vec<String> = obj.keys().cloned().collect();
        for k in keys {
            if !known.contains(&k) {
                obj.remove(&k);
                repairs.push(Repair {
                    from: k,
                    to: String::new(),
                    reason: "unknown_key".into(),
                });
            }
        }
        for k in known {
            if let Some(ty) = prop_type(spec, &k) {
                if let Some(v) = obj.get(&k).cloned() {
                    if let Some(c) = coerce(&v, &ty) {
                        if c != v {
                            repairs.push(Repair {
                                from: format!("{k}:{v}"),
                                to: format!("{k}:{c}"),
                                reason: "coerce".into(),
                            });
                            obj.insert(k, c);
                        }
                    }
                }
            }
        }
        for req in required_fields(spec) {
            let missing = match obj.get(&req) {
                None => true,
                Some(Value::Null) => true,
                Some(Value::String(s)) if s.is_empty() => true,
                _ => false,
            };
            if missing {
                return RepairResult {
                    name,
                    input: Value::Object(obj),
                    repairs,
                    reject: Some(format!("repair: missing field {req} — resend")),
                };
            }
        }
    }

    RepairResult {
        name,
        input: Value::Object(obj),
        repairs,
        reject: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaps_read_alias() {
        let r = repair("Read", &json!({"path": "a.txt"}));
        assert_eq!(r.name, "read");
        assert!(r.repairs.iter().any(|x| x.reason == "alias"));
        assert!(r.reject.is_none());
    }

    #[test]
    fn missing_path_rejects() {
        let r = repair("read", &json!({}));
        assert_eq!(r.reject.as_deref(), Some("repair: missing field path — resend"));
    }

    #[test]
    fn coerces_start_string() {
        let r = repair("read", &json!({"path": "a.txt", "start": "12"}));
        assert_eq!(r.input["start"], json!(12));
        assert!(r.repairs.iter().any(|x| x.reason == "coerce"));
    }

    #[test]
    fn unwraps_fenced_json() {
        let raw = "```json\n{\"path\":\"a.txt\"}\n```";
        let r = repair("read", &json!(raw));
        assert_eq!(r.input["path"], "a.txt");
    }

    #[test]
    fn bash_alias_shell() {
        let r = repair("Shell", &json!({"command": "echo hi"}));
        assert_eq!(r.name, "bash");
        assert!(r.reject.is_none());
    }
}
