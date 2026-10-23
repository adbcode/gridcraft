//! Bounded adapter for a host controlling the *live* workbook.
//!
//! Keep one adapter for the lifetime of a mounted editor. Call [`BridgeState::observe`] after
//! UI frames as well as before requests: its token covers workbook snapshots (including explicit
//! recalculation), document/sheet switches, selection and edit mode. It is deliberately more
//! conservative than `DocState::revision`, and is not a database snapshot version.
//!
//! Bounded edits and generic execution of one agent-capable command are exposed. Generic
//! execution is not a transaction and does not promise rollback for multi-action commands.
//! Undo means the last native operation, which may be a user's edit. The host must also refuse
//! requests while its cell editor or a dialog is open.

use std::collections::BTreeMap;
use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{CalcMode, Workbook};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value as Json, json};

use crate::{CommandInfo, DocState, Mode, Selection, Session, agent_command_specs, is_agent_command};

pub const MAX_CELLS: u64 = 1_000;
pub const MAX_REQUEST_BYTES: usize = 65_536;
const MAX_TEXT_BYTES: usize = 16_384;
const MAX_RESPONSE_BYTES: usize = 262_144;
const MAX_REQUESTS: usize = 64;
const MAX_SNAPSHOTS: usize = 64;
const MAX_PENDING_SNAPSHOTS: usize = 4;
const MAX_TOKEN: u64 = 9_007_199_254_740_991;
const BROWSER_SHELL_COMMANDS: &[&str] = &[
    "app.commands",
    "file.open",
    "file.new",
    "file.newWorkbook",
    "file.close",
    "file.save",
    "file.saveAs",
    "file.exportCsv",
    "file.exportHtml",
    "file.saveBytes",
    "window.activate",
];

pub type Result<T> = std::result::Result<T, BridgeError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct BridgeError {
    pub code: &'static str,
    pub message: String,
}

fn error(code: &'static str, message: impl Into<String>) -> BridgeError {
    BridgeError { code, message: message.into() }
}

/// All fields must be echoed unchanged for an inspection or mutation. Sheet identity is its
/// index and exact name *within this token*; renames/reordering invalidate the whole token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub document_id: u64,
    pub sheet_index: usize,
    pub sheet_name: String,
    pub change_token: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Context {
    pub identity: Identity,
    pub selection: String,
    pub active_cell: String,
    pub dirty: bool,
    pub undo_available: bool,
    pub ready: bool,
    pub calculation_mode: String,
}

struct Observed {
    document_id: u64,
    workbook: Arc<Workbook>,
    selection: Selection,
    mode: Mode,
}

#[derive(Clone)]
struct Receipt {
    method: String,
    params: Json,
    outcome: Result<Json>,
}

/// Immutable workbook to export. Retain this exact Arc when producing the uploaded bytes.
#[derive(Clone, Debug)]
pub struct CapturedSnapshot {
    pub snapshot_id: String,
    pub identity: Identity,
    pub workbook: Arc<Workbook>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SnapshotAck {
    pub snapshot_id: String,
    pub document_id: u64,
    /// True only when the currently open document still equals the captured snapshot.
    pub clean: bool,
}

struct SnapshotRecord {
    identity: Identity,
    workbook: Option<Arc<Workbook>>,
    sequence: u64,
    acknowledgement: Option<SnapshotAck>,
}

#[derive(Default)]
pub struct BridgeState {
    observed: Option<Observed>,
    token: u64,
    receipts: BTreeMap<String, Receipt>,
    snapshots: BTreeMap<String, SnapshotRecord>,
    snapshot_sequence: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectParams {
    expected: Identity,
    range: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MutationParams {
    request_id: String,
    expected: Identity,
    range: String,
    #[serde(default)]
    values: Option<Vec<Vec<Json>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteParams {
    request_id: String,
    expected: Identity,
    command: String,
    params: Json,
}

impl BridgeState {
    /// Observe even when no workbook is open so closing and reopening cannot revive a token.
    pub fn observe(&mut self, session: &Session) -> Result<Context> {
        let Some(doc) = session.active() else {
            if self.observed.take().is_some() {
                self.advance_token()?;
            }
            return Err(error("no_document", "no workbook is open"));
        };
        self.observe_document(doc, session.mode)
    }

    fn observe_document(&mut self, doc: &DocState, mode: Mode) -> Result<Context> {
        let changed = self.observed.as_ref().is_none_or(|old| {
            old.document_id != doc.uid || !Arc::ptr_eq(&old.workbook, &doc.wb) || old.selection != doc.selection || old.mode != mode
        });
        if changed {
            self.advance_token()?;
            // Keeping a strong reference ensures Arc::make_mut cannot hide later changes.
            self.observed = Some(Observed { document_id: doc.uid, workbook: doc.wb.clone(), selection: doc.selection.clone(), mode });
        }
        let sheet = doc.wb.active().ok_or_else(|| error("no_sheet", "the workbook has no active sheet"))?;
        Ok(Context {
            identity: Identity { document_id: doc.uid, sheet_index: doc.wb.active_sheet, sheet_name: sheet.name.clone(), change_token: self.token },
            selection: doc.selection.a1(),
            active_cell: doc.selection.active.a1(),
            dirty: doc.is_dirty(),
            undo_available: !doc.undo.is_empty(),
            ready: mode == Mode::Ready,
            calculation_mode: format!("{:?}", doc.wb.calc.mode),
        })
    }

    fn advance_token(&mut self) -> Result<()> {
        self.token =
            self.token.checked_add(1).filter(|v| *v <= MAX_TOKEN).ok_or_else(|| error("limit", "change token exhausted; reopen the editor"))?;
        Ok(())
    }

    fn check(&mut self, session: &Session, expected: &Identity) -> Result<Context> {
        let current = self.observe(session)?;
        if &current.identity != expected {
            return Err(error("conflict", "workbook, sheet, selection or calculation changed; inspect again before applying"));
        }
        Ok(current)
    }

    /// Methods: `context` {}, `commands` {}, `inspect` {expected, range}, `set_values`
    /// {request_id, expected, range, values}, `undo` {request_id, expected, range}, and
    /// `execute` {request_id, expected, command, params}. Generic execute is one command, not
    /// an atomic transaction. The range must name exactly the cells supplied, e.g. A1:B2.
    pub fn handle(&mut self, session: &mut Session, method: &str, params: Json) -> Result<Json> {
        bounded_json(&params, MAX_REQUEST_BYTES)?;
        match method {
            "context" => {
                if params.as_object().is_none_or(|p| !p.is_empty()) {
                    return Err(error("invalid_params", "context requires an empty object"));
                }
                encode(self.observe(session)?)
            }
            "commands" => {
                if params.as_object().is_none_or(|p| !p.is_empty()) {
                    return Err(error("invalid_params", "commands requires an empty object"));
                }
                let commands: Vec<CommandInfo> =
                    agent_command_specs().filter(|spec| browser_command_allowed(spec.id)).map(|spec| spec.info(session)).collect();
                encode(commands)
            }
            "inspect" => {
                let p: InspectParams = decode(params)?;
                let range = bounded_range(&p.range)?;
                let context = self.check(session, &p.expected)?;
                read_range(session, range, &p.expected, context)
            }
            "set_values" | "undo" => self.mutate(session, method, params),
            "execute" => self.execute_command(session, params),
            _ => Err(error("unknown_method", "supported methods are context, commands, inspect, set_values, undo and execute")),
        }
    }

    fn execute_command(&mut self, session: &mut Session, params: Json) -> Result<Json> {
        let p: ExecuteParams = decode(params.clone())?;
        valid_id(&p.request_id)?;
        if let Some(receipt) = self.receipts.get(&p.request_id) {
            return if receipt.method == "execute" && receipt.params == params {
                receipt.outcome.clone()
            } else {
                Err(error("duplicate_id", "request_id was already used with different parameters"))
            };
        }
        if self.receipts.len() >= MAX_REQUESTS {
            return Err(error("limit", "request receipt limit reached; reopen the editor before issuing new request IDs"));
        }
        let outcome = self.apply_execute(session, &p);
        self.receipts.insert(p.request_id, Receipt { method: "execute".into(), params, outcome: outcome.clone() });
        outcome
    }

    fn apply_execute(&mut self, session: &mut Session, p: &ExecuteParams) -> Result<Json> {
        if !browser_command_allowed(&p.command) || !is_agent_command(&p.command) {
            return Err(error("unsupported_command", format!("command `{}` is not available in the browser agent bridge", p.command)));
        }
        if p.params.as_object().is_none() {
            return Err(error("invalid_params", "execute `params` must be an object"));
        }
        if p.params.get("path").is_some() {
            return Err(error("invalid_params", "browser commands cannot use filesystem `path`; use byte imports or exports"));
        }
        let current = self.check(session, &p.expected)?;
        if !current.ready {
            return Err(error("busy", "finish or cancel the current cell edit before applying an action"));
        }
        if self.token >= MAX_TOKEN {
            return Err(error("limit", "change token exhausted; reopen the editor"));
        }
        let command_result = session.execute_headless(&p.command, p.params.clone()).map_err(|e| error("command_failed", e.to_string()))?;
        match self.observe(session) {
            Ok(context) => encode(json!({"request_id": p.request_id, "command_result": command_result, "context": context})),
            Err(e) => encode(json!({
                "request_id": p.request_id,
                "command_result": command_result,
                "applied": true,
                "verification_error": e
            })),
        }
    }

    fn mutate(&mut self, session: &mut Session, method: &str, params: Json) -> Result<Json> {
        let p: MutationParams = decode(params.clone())?;
        valid_id(&p.request_id)?;
        // Replay precedes conflict/busy checks: a transport retry returns the original outcome,
        // including its original token and readback, rather than claiming to describe today.
        if let Some(receipt) = self.receipts.get(&p.request_id) {
            return if receipt.method == method && receipt.params == params {
                receipt.outcome.clone()
            } else {
                Err(error("duplicate_id", "request_id was already used with different parameters"))
            };
        }
        if self.receipts.len() >= MAX_REQUESTS {
            return Err(error("limit", "mutation receipt limit reached; reopen the editor before issuing new request IDs"));
        }
        let outcome = self.apply(session, method, &p);
        self.receipts.insert(p.request_id, Receipt { method: method.to_string(), params, outcome: outcome.clone() });
        outcome
    }

    fn apply(&mut self, session: &mut Session, method: &str, p: &MutationParams) -> Result<Json> {
        let range = bounded_range(&p.range)?;
        let current = self.check(session, &p.expected)?;
        if !current.ready {
            return Err(error("busy", "finish or cancel the current cell edit before applying an action"));
        }
        // Reserve the next token before mutation so exhaustion cannot hide a completed edit.
        if self.token >= MAX_TOKEN {
            return Err(error("limit", "change token exhausted; reopen the editor"));
        }
        let (command, command_params) = if method == "set_values" {
            let values = p.values.as_ref().ok_or_else(|| error("invalid_params", "set_values requires a rectangular values array"))?;
            validate_values(values, range)?;
            if session.active().is_some_and(|doc| doc.wb.calc.mode == CalcMode::Manual) {
                return Err(error(
                    "manual_calculation",
                    "enable automatic calculation before an agent edit so readback contains recalculated values",
                ));
            }
            ("range.setValues", json!({"sheet": p.expected.sheet_index, "range": range.a1(), "values": values}))
        } else {
            if p.values.is_some() {
                return Err(error("invalid_params", "undo does not accept values"));
            }
            ("edit.undo", json!({"steps": 1}))
        };
        // The native command guard restores cell content after an escaped panic. Keep the
        // entire document here as well so a failed undo cannot consume history or move selection.
        let before = session.active().cloned().ok_or_else(|| error("no_document", "no workbook is open"))?;
        let command_result = match session.execute(command, command_params) {
            Ok(result) => result,
            Err(e) => {
                if let Some(doc) = session.active_mut() {
                    *doc = before;
                }
                return Err(error("command_failed", e.to_string()));
            }
        };
        let context = match self.observe(session) {
            Ok(context) => context,
            Err(e) => return Ok(json!({"request_id": p.request_id, "applied": true, "command_result": command_result, "verification_error": e})),
        };
        // An undo can restore a different active sheet. Read back the explicitly named original
        // sheet if it still exists, never silently substitute the new active sheet.
        let verification = read_range(session, range, &p.expected, context.clone());
        Ok(match verification {
            Ok(readback) => json!({"request_id": p.request_id, "applied": true, "command_result": command_result, "readback": readback}),
            Err(e) => {
                json!({"request_id": p.request_id, "applied": true, "command_result": command_result, "context": context, "verification_error": e})
            }
        })
    }

    /// Capture once, export `workbook`, persist, then acknowledge this exact ID. This does not
    /// mark anything saved. IDs and replay tombstones live until the mounted editor is replaced.
    pub fn capture_snapshot(&mut self, session: &Session, snapshot_id: &str, expected: &Identity) -> Result<CapturedSnapshot> {
        valid_id(snapshot_id)?;
        if let Some(record) = self.snapshots.get(snapshot_id) {
            if &record.identity != expected {
                return Err(error("duplicate_id", "snapshot_id already identifies a different capture"));
            }
            let workbook = record.workbook.clone().ok_or_else(|| error("snapshot_acknowledged", "this snapshot was already acknowledged"))?;
            return Ok(CapturedSnapshot { snapshot_id: snapshot_id.into(), identity: expected.clone(), workbook });
        }
        let context = self.check(session, expected)?;
        if !context.ready {
            return Err(error("busy", "finish or cancel the current cell edit before saving"));
        }
        let workbook = session.active().ok_or_else(|| error("no_document", "no workbook is open"))?.wb.clone();
        self.register_snapshot(snapshot_id, expected.clone(), workbook)
    }

    /// For a synchronous UI save callback that already has the exact document being exported.
    /// The caller must finish/cancel any pending cell editor before invoking this helper.
    pub fn capture_document_snapshot(&mut self, document: &DocState, snapshot_id: &str) -> Result<CapturedSnapshot> {
        valid_id(snapshot_id)?;
        if self.snapshots.contains_key(snapshot_id) {
            return Err(error("duplicate_id", "snapshot_id already identifies a capture"));
        }
        let context = self.observe_document(document, Mode::Ready)?;
        self.register_snapshot(snapshot_id, context.identity, document.wb.clone())
    }

    fn register_snapshot(&mut self, snapshot_id: &str, identity: Identity, workbook: Arc<Workbook>) -> Result<CapturedSnapshot> {
        if self.snapshots.len() >= MAX_SNAPSHOTS || self.snapshots.values().filter(|s| s.workbook.is_some()).count() >= MAX_PENDING_SNAPSHOTS {
            return Err(error("limit", "snapshot limit reached; acknowledge pending captures or reopen the editor"));
        }
        self.snapshot_sequence = self.snapshot_sequence.checked_add(1).ok_or_else(|| error("limit", "snapshot sequence exhausted"))?;
        self.snapshots.insert(
            snapshot_id.into(),
            SnapshotRecord { identity: identity.clone(), workbook: Some(workbook.clone()), sequence: self.snapshot_sequence, acknowledgement: None },
        );
        Ok(CapturedSnapshot { snapshot_id: snapshot_id.into(), identity, workbook })
    }

    /// Release the capture after export/upload failure. The ID remains consumed, preventing a
    /// delayed acknowledgement from marking a later or different workbook snapshot as saved.
    pub fn discard_snapshot(&mut self, snapshot_id: &str) -> Result<()> {
        let record = self.snapshots.get_mut(snapshot_id).ok_or_else(|| error("unknown_snapshot", "snapshot_id was not captured by this editor"))?;
        if record.acknowledgement.is_none() {
            record.workbook = None;
        }
        Ok(())
    }

    /// Call only after durable persistence of the bytes exported from the captured Arc. A newer
    /// acknowledged capture prevents delayed acknowledgements from moving the saved baseline back.
    pub fn acknowledge_snapshot(&mut self, session: &mut Session, snapshot_id: &str) -> Result<SnapshotAck> {
        valid_id(snapshot_id)?;
        let record = self.snapshots.get(snapshot_id).ok_or_else(|| error("unknown_snapshot", "snapshot_id was not captured by this editor"))?;
        if let Some(ack) = &record.acknowledgement {
            return Ok(ack.clone());
        }
        let uid = record.identity.document_id;
        let sequence = record.sequence;
        if self.snapshots.values().any(|s| s.identity.document_id == uid && s.sequence > sequence && s.acknowledgement.is_some()) {
            if let Some(old) = self.snapshots.get_mut(snapshot_id) {
                old.workbook = None;
            }
            return Err(error("superseded_snapshot", "a newer snapshot has already been acknowledged"));
        }
        let workbook = record.workbook.clone().ok_or_else(|| error("superseded_snapshot", "this snapshot was superseded"))?;
        let doc = session.docs.iter_mut().find(|d| d.uid == uid).ok_or_else(|| error("no_document", "the captured workbook is no longer open"))?;
        doc.saved = workbook;
        let ack = SnapshotAck { snapshot_id: snapshot_id.into(), document_id: uid, clean: !doc.is_dirty() };
        if let Some(record) = self.snapshots.get_mut(snapshot_id) {
            record.workbook = None;
            record.acknowledgement = Some(ack.clone());
        }
        Ok(ack)
    }
}

fn browser_command_allowed(id: &str) -> bool {
    !BROWSER_SHELL_COMMANDS.contains(&id)
}

fn valid_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 128 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c)) {
        return Err(error("invalid_params", "IDs must contain 1 to 128 ASCII letters, digits, '-', '_', '.' or ':'"));
    }
    Ok(())
}

fn decode<T: DeserializeOwned>(value: Json) -> Result<T> {
    serde_json::from_value(value).map_err(|e| error("invalid_params", e.to_string()))
}

fn encode(value: impl Serialize) -> Result<Json> {
    serde_json::to_value(value).map_err(|e| error("serialization", e.to_string()))
}

/// Bound serialized size without first allocating a second potentially enormous JSON string.
fn bounded_json(value: &Json, limit: usize) -> Result<()> {
    fn shape(value: &Json, depth: usize, nodes: &mut usize) -> Result<()> {
        *nodes = nodes.saturating_add(1);
        if depth > 16 || *nodes > 10_000 {
            return Err(error("limit", "JSON nesting or item limit exceeded"));
        }
        match value {
            Json::Array(values) => {
                for child in values {
                    shape(child, depth + 1, nodes)?;
                }
            }
            Json::Object(values) => {
                for child in values.values() {
                    shape(child, depth + 1, nodes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    shape(value, 0, &mut 0)?;
    struct LimitedWriter {
        used: usize,
        limit: usize,
    }
    impl std::io::Write for LimitedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.used = self.used.saturating_add(buf.len());
            if self.used > self.limit {
                return Err(std::io::Error::other("JSON size limit exceeded"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(LimitedWriter { used: 0, limit }, value).map_err(|_| error("limit", "JSON size limit exceeded"))
}

fn bounded_range(text: &str) -> Result<RangeRef> {
    if text.len() > 40 {
        return Err(error("invalid_params", "range must be a bounded A1 rectangle without a sheet prefix"));
    }
    let range = RangeRef::parse(text).ok_or_else(|| error("invalid_params", "invalid A1 range"))?;
    if range.count() > MAX_CELLS {
        return Err(error("limit", "range exceeds 1,000 cells"));
    }
    Ok(range)
}

fn validate_values(values: &[Vec<Json>], range: RangeRef) -> Result<()> {
    if values.len() != range.height() as usize || values.iter().any(|row| row.len() != range.width() as usize) {
        return Err(error("invalid_params", "values must be rectangular and match the exact range dimensions"));
    }
    for value in values.iter().flatten() {
        match value {
            Json::Null | Json::Bool(_) | Json::Number(_) => {}
            Json::String(text) if text.len() <= MAX_TEXT_BYTES => {}
            _ => return Err(error("invalid_params", "cells accept only null, numbers, booleans or strings up to 16,384 bytes")),
        }
    }
    Ok(())
}

fn read_range(session: &Session, range: RangeRef, target: &Identity, context: Context) -> Result<Json> {
    let doc = session.active().ok_or_else(|| error("no_document", "no workbook is open"))?;
    if doc.uid != target.document_id {
        return Err(error("conflict", "the original document is no longer active"));
    }
    let sheet = doc
        .wb
        .sheets
        .get(target.sheet_index)
        .filter(|s| s.name == target.sheet_name)
        .ok_or_else(|| error("conflict", "the original sheet no longer exists at the expected position"))?;
    let mut cells = Vec::new();
    let mut text_bytes = 0usize;
    for at in range.iter() {
        let value = scalar_json(sheet.value_ref(at).unwrap_or(&Value::Empty))?;
        let formula = if crate::display::formula_hidden(&doc.wb, sheet, at) {
            None
        } else {
            sheet.cell(at).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str())
        };
        text_bytes = text_bytes.saturating_add(value.as_str().map_or(0, str::len)).saturating_add(formula.map_or(0, str::len));
        if text_bytes > MAX_RESPONSE_BYTES || formula.is_some_and(|f| f.len() > MAX_TEXT_BYTES) {
            return Err(error("limit", "readback text exceeds the response limit; inspect a smaller range"));
        }
        cells.push(json!({"cell": CellRef::new(at.row, at.col).a1(), "value": value, "formula": formula.map(|f| format!("={f}"))}));
    }
    let result = json!({"context": context, "sheet_index": target.sheet_index, "sheet_name": target.sheet_name, "range": range.a1(), "cells": cells});
    bounded_json(&result, MAX_RESPONSE_BYTES)?;
    Ok(result)
}

fn scalar_json(value: &Value) -> Result<Json> {
    Ok(match value {
        Value::Empty => Json::Null,
        Value::Number(n) => json!(n),
        Value::Bool(b) => json!(b),
        Value::Text(text) if text.len() <= MAX_TEXT_BYTES => json!(text.as_ref()),
        Value::Text(_) => return Err(error("limit", "cell text exceeds 16,384 bytes")),
        Value::Error(e) => json!({"error": e.as_str()}),
        // Spill cells normally contain scalar values. Refuse nested arrays rather than expanding
        // an unbounded formula result through a small requested rectangle.
        Value::Array(_) => return Err(error("unsupported_value", "inspect spilled scalar cells instead of an array-valued cell")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Session, BridgeState) {
        let mut session = Session::new();
        session.new_workbook();
        (session, BridgeState::default())
    }

    fn identity(bridge: &mut BridgeState, session: &Session) -> Identity {
        bridge.observe(session).unwrap().identity
    }

    fn mutation(id: &str, expected: &Identity, range: &str, values: Json) -> Json {
        json!({"request_id": id, "expected": expected, "range": range, "values": values})
    }

    fn set_manual(session: &mut Session, cell: &str, input: &str) {
        session.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }

    #[test]
    fn live_manual_edit_formula_readback_and_single_native_undo() {
        let (mut session, mut bridge) = setup();
        let initial = identity(&mut bridge, &session);
        set_manual(&mut session, "A1", "7");
        assert_eq!(bridge.handle(&mut session, "inspect", json!({"expected": initial, "range": "A1"})).unwrap_err().code, "conflict");
        let expected = identity(&mut bridge, &session);
        let read = bridge.handle(&mut session, "inspect", json!({"expected": expected, "range": "A1"})).unwrap();
        assert_eq!(read["cells"][0]["value"], 7.0);
        let outcome = bridge.handle(&mut session, "set_values", mutation("formula", &expected, "B1:C1", json!([["=A1*2", "=B1+1"]]))).unwrap();
        assert_eq!(outcome["readback"]["cells"][0]["value"], 14.0);
        assert_eq!(outcome["readback"]["cells"][1]["value"], 15.0);
        assert_eq!(outcome["readback"]["cells"][0]["formula"], "=A1*2");
        assert_eq!(session.doc().unwrap().undo.len(), 2);
        let expected = identity(&mut bridge, &session);
        let undone = bridge.handle(&mut session, "undo", json!({"request_id": "undo", "expected": expected, "range": "A1:C1"})).unwrap();
        assert_eq!(undone["readback"]["cells"][0]["value"], 7.0);
        assert_eq!(undone["readback"]["cells"][1]["value"], Json::Null);
        assert_eq!(undone["readback"]["cells"][2]["value"], Json::Null);
        assert!(session.ui_requests.is_empty());
    }

    #[test]
    fn retries_replay_original_mutation_and_undo_even_after_manual_edits() {
        let (mut session, mut bridge) = setup();
        let expected = identity(&mut bridge, &session);
        let request = mutation("write", &expected, "A1", json!([[42]]));
        let written = bridge.handle(&mut session, "set_values", request.clone()).unwrap();
        set_manual(&mut session, "A1", "99");
        assert_eq!(bridge.handle(&mut session, "set_values", request.clone()).unwrap(), written);
        assert_eq!(session.doc().unwrap().undo.len(), 2);
        assert_eq!(session.doc().unwrap().wb.active().unwrap().value(CellRef::default()), Value::number(99.0));
        let mut altered = request;
        altered["values"] = json!([[43]]);
        assert_eq!(bridge.handle(&mut session, "set_values", altered).unwrap_err().code, "duplicate_id");
        let expected = identity(&mut bridge, &session);
        let undo = json!({"request_id": "undo", "expected": expected, "range": "A1"});
        let undone = bridge.handle(&mut session, "undo", undo.clone()).unwrap();
        assert_eq!(undone["readback"]["cells"][0]["value"], 42.0);
        assert_eq!(bridge.handle(&mut session, "undo", undo).unwrap(), undone);
        assert_eq!(session.doc().unwrap().undo.len(), 1);
    }

    #[test]
    fn token_covers_selection_recalculation_sheets_and_document_switches() {
        let (mut session, mut bridge) = setup();
        let initial = identity(&mut bridge, &session);
        let revision = session.doc().unwrap().revision;
        session.execute("selection.set", json!({"range": "C7:D8"})).unwrap();
        let selected = identity(&mut bridge, &session);
        assert!(selected.change_token > initial.change_token);
        assert_eq!(session.doc().unwrap().revision, revision);
        session.execute("formulas.calculateNow", json!({})).unwrap();
        let recalculated = identity(&mut bridge, &session);
        assert!(recalculated.change_token > selected.change_token);
        assert_eq!(session.doc().unwrap().revision, revision);
        session.execute("home.insertSheet", json!({"name": "Other"})).unwrap();
        let inserted = identity(&mut bridge, &session);
        session.execute("selection.set", json!({"sheet": "Sheet1", "range": "A1"})).unwrap();
        let switched = identity(&mut bridge, &session);
        assert!(switched.change_token > inserted.change_token);
        assert_eq!(switched.sheet_name, "Sheet1");
        session.execute("sheet.rename", json!({"name": "Renamed"})).unwrap();
        let renamed = identity(&mut bridge, &session);
        assert_eq!(renamed.sheet_name, "Renamed");
        session.new_workbook();
        let new_document = identity(&mut bridge, &session);
        assert_ne!(new_document.document_id, renamed.document_id);
        session.set_active(0);
        let returned = identity(&mut bridge, &session);
        assert_eq!(returned.document_id, renamed.document_id);
        assert!(returned.change_token > renamed.change_token);
        assert_eq!(bridge.handle(&mut session, "set_values", mutation("stale", &renamed, "A1", json!([[1]]))).unwrap_err().code, "conflict");
    }

    #[test]
    fn opened_workbook_gets_new_identity_and_stale_edits_cannot_target_it() {
        let (mut session, mut bridge) = setup();
        let initial = identity(&mut bridge, &session);
        let workbook = gridcraft_xlsx::write_xlsx(&Workbook::new()).unwrap();
        session.execute("file.open", json!({"name": "opened.xlsx", "base64": crate::io::base64_encode(&workbook)})).unwrap();
        let opened = identity(&mut bridge, &session);
        assert_ne!(opened.document_id, initial.document_id);
        assert_eq!(bridge.handle(&mut session, "set_values", mutation("old", &initial, "A1", json!([[8]]))).unwrap_err().code, "conflict");
        assert!(session.doc().unwrap().wb.active().unwrap().value(CellRef::default()).is_empty());
        assert!(session.ui_requests.is_empty());
    }

    #[test]
    fn empty_and_hostile_parameters_never_open_dialogs_or_mutate() {
        let (mut session, mut bridge) = setup();
        let expected = identity(&mut bridge, &session);
        let workbook = session.doc().unwrap().wb.clone();
        for method in ["inspect", "set_values", "undo", "file.open", "file.save", "arbitrary"] {
            assert!(bridge.handle(&mut session, method, json!({})).is_err(), "{method}");
        }
        for (i, range) in ["A:XFD", "1:1048576", "A0", "XFE1", "Sheet1!A1", "A1,A2", "A1:A1001"].iter().enumerate() {
            assert!(bridge.handle(&mut session, "set_values", mutation(&format!("range-{i}"), &expected, range, json!([[1]]))).is_err());
        }
        for (i, values) in [json!([]), json!([[1], [2, 3]]), json!([[{"x": 1}]]), json!([["x".repeat(MAX_TEXT_BYTES + 1)]])].into_iter().enumerate() {
            assert!(bridge.handle(&mut session, "set_values", mutation(&format!("shape-{i}"), &expected, "A1", values)).is_err());
        }
        assert!(session.ui_requests.is_empty());
        assert!(Arc::ptr_eq(&workbook, &session.doc().unwrap().wb));
        assert!(session.doc().unwrap().undo.is_empty());
    }

    #[test]
    fn unfinished_edit_is_busy_and_failure_retries_are_deterministic() {
        let (mut session, mut bridge) = setup();
        session.mode = Mode::Edit;
        let expected = identity(&mut bridge, &session);
        let request = mutation("busy", &expected, "A1", json!([[1]]));
        let failure = bridge.handle(&mut session, "set_values", request.clone()).unwrap_err();
        assert_eq!(failure.code, "busy");
        session.mode = Mode::Ready;
        assert_eq!(bridge.handle(&mut session, "set_values", request).unwrap_err(), failure);
        assert!(session.doc().unwrap().undo.is_empty());
    }

    #[test]
    fn exhausted_ledger_refuses_new_mutations_but_keeps_old_receipts() {
        let (mut session, mut bridge) = setup();
        let first_expected = identity(&mut bridge, &session);
        let first_request = mutation("write-0", &first_expected, "A1", json!([[0]]));
        let first = bridge.handle(&mut session, "set_values", first_request.clone()).unwrap();
        for i in 1..MAX_REQUESTS {
            let expected = identity(&mut bridge, &session);
            bridge.handle(&mut session, "set_values", mutation(&format!("write-{i}"), &expected, "A1", json!([[i]]))).unwrap();
        }
        let expected = identity(&mut bridge, &session);
        assert_eq!(bridge.handle(&mut session, "set_values", mutation("overflow", &expected, "A1", json!([[123]]))).unwrap_err().code, "limit");
        assert_eq!(bridge.handle(&mut session, "set_values", first_request).unwrap(), first);
        assert_eq!(session.doc().unwrap().undo.len(), MAX_REQUESTS);
    }

    #[test]
    fn snapshot_acknowledges_exact_bytes_and_preserves_newer_unsaved_changes() {
        let (mut session, mut bridge) = setup();
        set_manual(&mut session, "A1", "1");
        let expected = identity(&mut bridge, &session);
        let captured = bridge.capture_snapshot(&session, "snapshot", &expected).unwrap();
        let repeated = bridge.capture_snapshot(&session, "snapshot", &expected).unwrap();
        assert!(Arc::ptr_eq(&captured.workbook, &repeated.workbook));
        assert!(session.doc().unwrap().is_dirty());
        let bytes = gridcraft_xlsx::write_xlsx(&captured.workbook).unwrap();
        set_manual(&mut session, "A1", "2");
        let ack = bridge.acknowledge_snapshot(&mut session, "snapshot").unwrap();
        assert!(!ack.clean);
        assert!(session.doc().unwrap().is_dirty());
        assert!(Arc::ptr_eq(&session.doc().unwrap().saved, &captured.workbook));
        let (reopened, _) = gridcraft_xlsx::read_xlsx(&bytes).unwrap();
        assert_eq!(reopened.active().unwrap().value(CellRef::default()), Value::number(1.0));
        assert!(!bridge.acknowledge_snapshot(&mut session, "snapshot").unwrap().clean);
        session.execute("edit.undo", json!({})).unwrap();
        assert!(!session.doc().unwrap().is_dirty());
    }

    #[test]
    fn out_of_order_snapshot_ack_cannot_revert_saved_baseline() {
        let (mut session, mut bridge) = setup();
        set_manual(&mut session, "A1", "1");
        let old = bridge.capture_document_snapshot(session.doc().unwrap(), "old").unwrap();
        set_manual(&mut session, "A1", "2");
        let new = bridge.capture_document_snapshot(session.doc().unwrap(), "new").unwrap();
        assert!(bridge.acknowledge_snapshot(&mut session, "new").unwrap().clean);
        assert_eq!(bridge.acknowledge_snapshot(&mut session, "old").unwrap_err().code, "superseded_snapshot");
        assert!(Arc::ptr_eq(&session.doc().unwrap().saved, &new.workbook));
        assert!(!Arc::ptr_eq(&old.workbook, &new.workbook));
        assert!(!session.doc().unwrap().is_dirty());
    }

    #[test]
    fn snapshot_ids_are_bound_to_capture_and_pending_memory_is_bounded() {
        let (mut session, mut bridge) = setup();
        let initial = identity(&mut bridge, &session);
        bridge.capture_snapshot(&session, "same", &initial).unwrap();
        set_manual(&mut session, "A1", "1");
        let next = identity(&mut bridge, &session);
        assert_eq!(bridge.capture_snapshot(&session, "same", &next).unwrap_err().code, "duplicate_id");
        for i in 1..MAX_PENDING_SNAPSHOTS {
            bridge.capture_snapshot(&session, &format!("pending-{i}"), &next).unwrap();
        }
        assert_eq!(bridge.capture_snapshot(&session, "overflow", &next).unwrap_err().code, "limit");
        bridge.discard_snapshot("same").unwrap();
        assert_eq!(bridge.acknowledge_snapshot(&mut session, "same").unwrap_err().code, "superseded_snapshot");
        bridge.capture_snapshot(&session, "after-discard", &next).unwrap();
        assert!(session.doc().unwrap().is_dirty());
    }

    #[test]
    fn successful_edit_with_unreadable_result_is_reported_as_applied() {
        let (mut session, mut bridge) = setup();
        let expected = identity(&mut bridge, &session);
        let request = mutation("large-result", &expected, "A1", json!([["=REPT(\"x\",17000)"]]));
        let result = bridge.handle(&mut session, "set_values", request.clone()).unwrap();
        assert_eq!(result["applied"], true);
        assert_eq!(result["verification_error"]["code"], "limit");
        assert_eq!(session.doc().unwrap().undo.len(), 1);
        assert_eq!(bridge.handle(&mut session, "set_values", request).unwrap(), result);
        assert_eq!(session.doc().unwrap().undo.len(), 1);
    }

    #[test]
    fn undo_that_removes_target_sheet_reports_applied_with_verification_error() {
        let (mut session, mut bridge) = setup();
        session.execute("home.insertSheet", json!({"name": "Other"})).unwrap();
        let expected = identity(&mut bridge, &session);
        assert_eq!(expected.sheet_name, "Other");
        let request = json!({"request_id": "undo-sheet", "expected": expected, "range": "A1"});
        let result = bridge.handle(&mut session, "undo", request.clone()).unwrap();
        assert_eq!(result["applied"], true);
        assert_eq!(result["verification_error"]["code"], "conflict");
        assert_eq!(result["context"]["identity"]["sheet_name"], "Sheet1");
        assert_eq!(session.doc().unwrap().wb.sheets.len(), 1);
        assert_eq!(bridge.handle(&mut session, "undo", request).unwrap(), result);
    }

    #[test]
    fn disabled_undo_preserves_whole_document_and_replays_failure() {
        let (mut session, mut bridge) = setup();
        let expected = identity(&mut bridge, &session);
        let workbook = session.doc().unwrap().wb.clone();
        let request = json!({"request_id": "empty-undo", "expected": expected, "range": "A1"});
        let failure = bridge.handle(&mut session, "undo", request.clone()).unwrap_err();
        assert_eq!(failure.code, "command_failed");
        assert!(Arc::ptr_eq(&session.doc().unwrap().wb, &workbook));
        set_manual(&mut session, "A1", "7");
        assert_eq!(bridge.handle(&mut session, "undo", request).unwrap_err(), failure);
        assert_eq!(session.doc().unwrap().undo.len(), 1);
    }

    #[test]
    fn manual_calculation_allows_inspection_but_refuses_unverified_agent_writes() {
        let (mut session, mut bridge) = setup();
        set_manual(&mut session, "A1", "7");
        session.execute("formulas.calculationOptions", json!({"mode": "manual"})).unwrap();
        let expected = identity(&mut bridge, &session);
        let before = session.doc().unwrap().wb.clone();
        let history = session.doc().unwrap().undo.len();
        let read = bridge.handle(&mut session, "inspect", json!({"expected": expected, "range": "A1"})).unwrap();
        assert_eq!(read["context"]["calculation_mode"], "Manual");
        assert_eq!(read["cells"][0]["value"], 7.0);
        let request = mutation("manual-formula", &expected, "B1", json!([["=A1*2"]]));
        assert_eq!(bridge.handle(&mut session, "set_values", request).unwrap_err().code, "manual_calculation");
        assert!(Arc::ptr_eq(&session.doc().unwrap().wb, &before));
        assert_eq!(session.doc().unwrap().undo.len(), history);
        assert!(session.ui_requests.is_empty());
    }

    #[test]
    fn generic_execute_formats_formulas_and_exposes_readback() {
        let (mut session, mut bridge) = setup();
        let expected = identity(&mut bridge, &session);
        let write = json!({
            "request_id": "formula",
            "expected": expected,
            "command": "cell.set",
            "params": {"cell": "A1", "input": "=1+2"}
        });
        let result = bridge.handle(&mut session, "execute", write).unwrap();
        assert_eq!(result["request_id"], "formula");
        assert!(result["command_result"].is_object());
        let expected = identity(&mut bridge, &session);
        bridge
            .handle(
                &mut session,
                "execute",
                json!({
                    "request_id": "format",
                    "expected": expected,
                    "command": "home.bold",
                    "params": {"range": "A1", "on": true}
                }),
            )
            .unwrap();
        let expected = identity(&mut bridge, &session);
        let read = bridge.handle(&mut session, "inspect", json!({"expected": expected, "range": "A1"})).unwrap();
        assert_eq!(read["cells"][0]["value"], 3.0);
        assert_eq!(read["cells"][0]["formula"], "=1+2");
        let doc = session.doc().unwrap();
        let style = doc.wb.styles.get(doc.wb.active().unwrap().style_id(CellRef::parse("A1").unwrap()));
        assert!(style.font.bold);
    }

    #[test]
    fn generic_execute_retries_once_and_rejects_stale_identity() {
        let (mut session, mut bridge) = setup();
        let expected = identity(&mut bridge, &session);
        let request = json!({
            "request_id": "generic-retry",
            "expected": expected,
            "command": "cell.set",
            "params": {"cell": "A1", "input": "42"}
        });
        let first = bridge.handle(&mut session, "execute", request.clone()).unwrap();
        let undo_count = session.doc().unwrap().undo.len();
        assert_eq!(bridge.handle(&mut session, "execute", request.clone()).unwrap(), first);
        assert_eq!(session.doc().unwrap().undo.len(), undo_count);
        let mut altered = request.clone();
        altered["params"]["input"] = json!("43");
        assert_eq!(bridge.handle(&mut session, "execute", altered).unwrap_err().code, "duplicate_id");

        let stale = expected;
        session.execute("selection.set", json!({"range": "B1"})).unwrap();
        let request = json!({
            "request_id": "generic-stale",
            "expected": stale,
            "command": "cell.set",
            "params": {"cell": "A1", "input": "99"}
        });
        assert_eq!(bridge.handle(&mut session, "execute", request).unwrap_err().code, "conflict");
        assert_eq!(session.doc().unwrap().wb.active().unwrap().value(CellRef::default()), Value::number(42.0));
    }

    #[test]
    fn browser_commands_hide_editor_actions_and_reject_paths_and_shell_commands() {
        let (mut session, mut bridge) = setup();
        let commands = bridge.handle(&mut session, "commands", json!({})).unwrap();
        let ids: Vec<&str> = commands.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
        assert!(!ids.contains(&"edit.beginEdit"));
        assert!(!ids.contains(&"formulas.insertFunction"));
        assert!(!ids.contains(&"formulas.useInFormula"));
        assert!(!ids.contains(&"link.open"));
        assert!(!ids.contains(&"file.saveBytes"));
        assert!(!ids.contains(&"file.open"));
        assert!(!ids.contains(&"file.save"));
        assert!(!ids.contains(&"file.saveAs"));
        assert!(!ids.contains(&"file.exportCsv"));
        assert!(!ids.contains(&"file.exportHtml"));
        assert!(!ids.contains(&"window.activate"));

        let expected = identity(&mut bridge, &session);
        let ui_only = json!({
            "request_id": "ui-only",
            "expected": expected,
            "command": "edit.beginEdit",
            "params": {}
        });
        assert_eq!(bridge.handle(&mut session, "execute", ui_only).unwrap_err().code, "unsupported_command");
        let name_editor = json!({
            "request_id": "name-editor",
            "expected": expected,
            "command": "formulas.useInFormula",
            "params": {"name": "Total"}
        });
        assert_eq!(bridge.handle(&mut session, "execute", name_editor).unwrap_err().code, "unsupported_command");
        let blocked = json!({
            "request_id": "shell-open",
            "expected": expected,
            "command": "file.open",
            "params": {"name": "Book.xlsx", "base64": ""}
        });
        assert_eq!(bridge.handle(&mut session, "execute", blocked).unwrap_err().code, "unsupported_command");
        let path = json!({
            "request_id": "path",
            "expected": expected,
            "command": "home.bold",
            "params": {"range": "A1", "path": "/tmp/workbook.xlsx"}
        });
        assert_eq!(bridge.handle(&mut session, "execute", path).unwrap_err().code, "invalid_params");
        assert!(session.doc().unwrap().undo.is_empty());
    }

    #[test]
    fn generic_execute_rejects_busy_and_missing_documents() {
        let (mut session, mut bridge) = setup();
        session.mode = Mode::Edit;
        let expected = identity(&mut bridge, &session);
        let busy = json!({
            "request_id": "busy",
            "expected": expected,
            "command": "cell.set",
            "params": {"cell": "A1", "input": "1"}
        });
        assert_eq!(bridge.handle(&mut session, "execute", busy).unwrap_err().code, "busy");

        let mut empty_session = Session::new();
        let mut empty_bridge = BridgeState::default();
        let no_document = json!({
            "request_id": "no-doc",
            "expected": {"document_id": 1, "sheet_index": 0, "sheet_name": "Sheet1", "change_token": 0},
            "command": "cell.set",
            "params": {"cell": "A1", "input": "1"}
        });
        assert_eq!(empty_bridge.handle(&mut empty_session, "execute", no_document).unwrap_err().code, "no_document");
    }
}
