//! Transport-neutral runtime for the agent-controlled browser shell.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![forbid(unsafe_code)]

use gridcraft_engine::agent_bridge::{BridgeState, Identity};
use gridcraft_engine::{DocState, Session, io};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_JSON_BYTES: usize = 65_536;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub result: Value,
    pub bytes: Option<Vec<u8>>,
}

struct Receipt {
    fingerprint: Vec<u8>,
    outcome: Result<Reply, String>,
}

pub struct Runtime {
    pub session: Session,
    bridge: BridgeState,
    receipts: HashMap<String, Receipt>,
    exports: usize,
}

impl Default for Runtime {
    fn default() -> Self {
        let mut session = Session::new();
        session.new_workbook();
        Self { session, bridge: BridgeState::default(), receipts: HashMap::new(), exports: 0 }
    }
}

impl Runtime {
    pub fn context(&mut self) -> Result<Value, String> {
        self.bridge.handle(&mut self.session, "context", json!({})).map_err(|e| e.to_string())
    }

    pub fn request(&mut self, request: Request, bytes: Option<&[u8]>) -> Result<Reply, String> {
        if request.id.is_empty() || request.id.len() > 128 || !request.id.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c)) {
            return Err("invalid request id".into());
        }
        let encoded = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if encoded.len() > MAX_JSON_BYTES || bytes.is_some_and(|b| b.len() > MAX_FILE_BYTES) {
            return Err("request exceeds 64 KiB JSON or 16 MiB file limit".into());
        }
        if !request.params.is_object() || (request.method != "open" && bytes.is_some()) {
            return Err("params must be an object; binary bytes are only accepted for open".into());
        }
        if matches!(request.method.as_str(), "context" | "commands" | "inspect") {
            return self.dispatch(&request, bytes);
        }
        let mut hash = Sha256::new();
        hash.update(encoded);
        hash.update(bytes.unwrap_or_default());
        let fingerprint = hash.finalize().to_vec();
        if let Some(old) = self.receipts.get(&request.id) {
            return if old.fingerprint == fingerprint {
                old.outcome.clone()
            } else {
                Err("duplicate_id: request id was used with different parameters or bytes".into())
            };
        }
        if self.receipts.len() >= 128 {
            return Err("request receipt limit reached; save and remount before issuing new mutations".into());
        }
        let outcome = self.dispatch(&request, bytes);
        self.receipts.insert(request.id.clone(), Receipt { fingerprint, outcome: outcome.clone() });
        outcome
    }

    fn expected(&mut self, params: &Value) -> Result<Identity, String> {
        let expected: Identity = serde_json::from_value(params["expected"].clone()).map_err(|e| format!("expected identity: {e}"))?;
        if expected != self.bridge.observe(&self.session).map_err(|e| e.to_string())?.identity {
            return Err("conflict: inspect current context before applying".into());
        }
        Ok(expected)
    }

    fn dispatch(&mut self, request: &Request, bytes: Option<&[u8]>) -> Result<Reply, String> {
        let mut params = request.params.clone();
        let result = match request.method.as_str() {
            "open" => {
                self.expected(&params)?;
                if self.session.active().is_some_and(|doc| doc.is_dirty()) && params["discardChanges"] != true {
                    return Err("unsaved_changes: save first or explicitly set discardChanges".into());
                }
                let name = params["name"].as_str().filter(|n| !n.is_empty() && n.len() <= 256).ok_or("open needs a name (1–256 bytes)")?;
                let bytes = bytes.ok_or("open needs binary bytes")?;
                let (workbook, warnings) =
                    io::open_bytes_with(name, bytes, params["password"].as_str(), params["csvAsText"] == true).map_err(|e| e.to_string())?;
                let document = DocState::new(workbook, None, name.into());
                while !self.session.documents().is_empty() {
                    self.session.close_document(0);
                }
                self.session.add_document(document);
                json!({"context": self.context()?, "warnings": warnings})
            }
            "snapshot.export" => {
                let expected = self.expected(&params)?;
                if self.exports >= 4 {
                    return Err("four binary exports per mounted host session; persist and remount to continue".into());
                }
                let captured = self.bridge.capture_snapshot(&self.session, &request.id, &expected).map_err(|e| e.to_string())?;
                let export = io::save_bytes(&captured.workbook, "Workbook.xlsx").map_err(|e| e.to_string());
                let bytes = match export {
                    Ok(bytes) if bytes.len() <= MAX_FILE_BYTES => bytes,
                    other => {
                        let _ = self.bridge.discard_snapshot(&request.id);
                        return Err(other.err().unwrap_or_else(|| "export exceeds 16 MiB".into()));
                    }
                };
                self.exports += 1;
                return Ok(Reply {
                    result: json!({"snapshot_id": request.id, "identity": captured.identity, "name": "Workbook.xlsx"}),
                    bytes: Some(bytes),
                });
            }
            "snapshot.ack" => {
                let id = params["snapshot_id"].as_str().ok_or("snapshot_id required")?;
                if params["storage_version"].as_str().is_none_or(|v| v.trim().is_empty() || v.len() > 256) {
                    return Err("storage_version must identify a successful durable save".into());
                }
                serde_json::to_value(self.bridge.acknowledge_snapshot(&mut self.session, id).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?
            }
            method => {
                if matches!(method, "execute" | "set_values" | "undo") {
                    if params.get("request_id").is_some_and(|v| v != request.id.as_str()) {
                        return Err("request_id must match the envelope id".into());
                    }
                    params["request_id"] = json!(request.id);
                }
                self.bridge.handle(&mut self.session, method, params).map_err(|e| e.to_string())?
            }
        };
        Ok(Reply { result, bytes: None })
    }

    pub fn preview(&mut self, range: &str) -> Result<Value, String> {
        let range = gridcraft_core::RangeRef::parse(range).ok_or("use an A1-style preview range")?;
        if range.count() > 1_000 {
            return Err("preview is limited to 1,000 cells; choose a smaller range".into());
        }
        self.session.execute_headless("sheet.read", json!({"range": range.a1(), "formatted": true})).map_err(|e| e.to_string())
    }

    pub fn download(&self) -> Result<Vec<u8>, String> {
        let bytes = io::save_bytes(&self.session.doc().map_err(|e| e.to_string())?.wb, "Workbook.xlsx").map_err(|e| e.to_string())?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err("download exceeds 16 MiB".into());
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn request(runtime: &mut Runtime, id: &str, method: &str, mut params: Value) -> Request {
        params["expected"] = runtime.context().unwrap()["identity"].clone();
        Request { id: id.into(), method: method.into(), params }
    }

    #[test]
    fn retries_do_not_repeat_edits() {
        let mut runtime = Runtime::default();
        let write = request(&mut runtime, "write", "execute", json!({"command":"cell.set", "params":{"cell":"A1", "input":"=1+2"}}));
        runtime.request(write.clone(), None).unwrap();
        let history = runtime.session.doc().unwrap().undo.len();
        runtime.request(write.clone(), None).unwrap();
        assert_eq!(runtime.session.doc().unwrap().undo.len(), history);
        assert_eq!(runtime.preview("A1").unwrap()["values"][0][0], "3");
        let mut different = write;
        different.params["params"]["input"] = json!(8);
        assert!(runtime.request(different, None).unwrap_err().contains("duplicate_id"));
    }

    #[test]
    fn acknowledgement_does_not_clean_later_edits() {
        let mut runtime = Runtime::default();
        let edit = request(&mut runtime, "a", "execute", json!({"command":"cell.set", "params":{"cell":"A1", "input":7}}));
        runtime.request(edit, None).unwrap();
        let export = request(&mut runtime, "save", "snapshot.export", json!({}));
        let saved = runtime.request(export.clone(), None).unwrap();
        assert!(runtime.session.doc().unwrap().is_dirty());
        let edit = request(&mut runtime, "b", "execute", json!({"command":"cell.set", "params":{"cell":"A1", "input":9}}));
        runtime.request(edit, None).unwrap();
        let ack = Request { id: "ack".into(), method: "snapshot.ack".into(), params: json!({"snapshot_id":"save", "storage_version":"v1"}) };
        assert_eq!(runtime.request(ack, None).unwrap().result["clean"], false);
        assert!(runtime.session.doc().unwrap().is_dirty());
        assert_eq!(runtime.request(export, None).unwrap().bytes, saved.bytes);
        let (workbook, _) = io::open_bytes("saved.xlsx", &saved.bytes.unwrap()).unwrap();
        let mut reopened = Session::new();
        reopened.add_document(DocState::new(workbook, None, "saved".into()));
        assert_eq!(reopened.execute_headless("sheet.read", json!({"range":"A1"})).unwrap()["values"][0][0], 7.0);
    }

    #[test]
    fn open_requires_current_identity_and_explicit_discard() {
        let mut runtime = Runtime::default();
        let stale = request(&mut runtime, "open1", "open", json!({"name":"a.csv"}));
        let edit = request(&mut runtime, "a", "execute", json!({"command":"cell.set", "params":{"cell":"A1", "input":7}}));
        runtime.request(edit, None).unwrap();
        assert!(runtime.request(stale, Some(b"A,B\n1,2")).unwrap_err().contains("conflict"));
        let blocked = request(&mut runtime, "open2", "open", json!({"name":"a.csv"}));
        assert!(runtime.request(blocked, Some(b"A,B\n1,2")).unwrap_err().contains("unsaved_changes"));
        let allowed = request(&mut runtime, "open3", "open", json!({"name":"a.csv", "discardChanges":true}));
        runtime.request(allowed.clone(), Some(b"A,B\n1,2")).unwrap();
        runtime.request(allowed, Some(b"A,B\n1,2")).unwrap();
        assert_eq!(runtime.session.documents().len(), 1);
        assert_eq!(runtime.preview("A2:B2").unwrap()["values"], json!([["1", "2"]]));
    }

    #[test]
    fn preview_is_bounded() {
        assert!(Runtime::default().preview("A1:XFD1048576").is_err());
    }
}
