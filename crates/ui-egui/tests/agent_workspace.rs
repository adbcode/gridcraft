//! Agent workspace behavior is exercised against the same live engine used by the grid.
use std::sync::Arc;

use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_ui_egui::{Services, SheetApp, editor::EditState};
use serde_json::json;

fn app() -> SheetApp {
    let mut session = Session::new();
    session.new_workbook();
    SheetApp::new(session, Services::default())
}

fn value(app: &SheetApp, cell: &str) -> Value {
    app.session.doc().unwrap().wb.active().unwrap().value(CellRef::parse(cell).unwrap())
}

#[test]
fn modes_keep_the_same_document_history_draft_and_pending_preview() {
    let mut app = app();
    app.run("cell.set", json!({"cell":"H1","input":"manual"})).unwrap();
    app.run("ui.agent.prompt", json!({"text":"Compare sales"})).unwrap();
    app.run("ui.agent.compare", json!({})).unwrap();
    let uid = app.session.doc().unwrap().uid;
    let workbook = app.session.doc().unwrap().wb.clone();
    let expected = app.agent.preview.as_ref().unwrap().expected.clone();
    for on in [true, false, true] {
        app.run("ui.agent.toggle", json!({"on":on})).unwrap();
        assert_eq!(app.agent.enabled, on);
        assert_eq!(app.session.doc().unwrap().uid, uid);
        assert_eq!(app.session.doc().unwrap().undo.len(), 1);
        assert!(Arc::ptr_eq(&workbook, &app.session.doc().unwrap().wb));
        assert_eq!(app.agent.preview.as_ref().unwrap().expected, expected);
        assert_eq!(app.agent.prompt, "Compare sales");
    }
    assert_eq!(app.session.documents().len(), 1);
}

#[test]
fn explicit_remembered_rule_changes_query_and_never_silently_saves_to_cloud() {
    let mut app = app();
    assert!(app.agent.rules.is_empty());
    app.run("ui.agent.submit", json!({"text":"Compare sales"})).unwrap();
    assert_eq!(app.agent.preview.as_ref().unwrap().review.total().current_minor, 65_500);
    app.run("ui.agent.submit", json!({"text":"Remember to exclude test orders"})).unwrap();
    assert!(app.agent.preview.is_none(), "changing rules invalidates the old query preview");
    assert_eq!(app.agent.rules.len(), 1);
    app.run("ui.agent.submit", json!({"text":"Compare weekly sales"})).unwrap();
    let review = &app.agent.preview.as_ref().unwrap().review;
    assert_eq!(review.total().current_minor, 15_500);
    assert_eq!(review.total().previous_minor, 15_000);
    assert_eq!(review.provenance().applied_rule_ids, ["exclude-tests-v1"]);
    assert_eq!(review.provenance().previous_record_count, 4);
    assert_eq!(review.provenance().current_record_count, 3);
    assert!(app.agent.messages.iter().any(|m| m.text.contains("lost when the app restarts")));
    assert!(app.session.doc().unwrap().undo.is_empty(), "preview and rules do not edit cells");
    assert!(app.run("ui.agent.submit", json!({"text":"Remember to exclude all shoes"})).is_err());
    assert_eq!(app.agent.rules.len(), 1);
    assert!(app.session.ui_requests.is_empty());
}

#[test]
fn manual_changes_during_preview_conflict_even_after_mode_switch() {
    let mut app = app();
    app.run("ui.agent.compare", json!({})).unwrap();
    app.run("ui.agent.toggle", json!({"on":false})).unwrap();
    app.run("cell.set", json!({"cell":"A1","input":"keep my edit"})).unwrap();
    app.run("ui.agent.toggle", json!({"on":true})).unwrap();
    let error = app.run("ui.agent.apply", json!({})).unwrap_err();
    assert!(error.contains("conflict"));
    assert_eq!(value(&app, "A1"), Value::text("keep my edit"));
    assert_eq!(app.session.doc().unwrap().undo.len(), 1);
    assert!(app.agent.receipts.is_empty());
    app.run("ui.agent.compare", json!({})).unwrap();
    assert!(app.run("ui.agent.apply", json!({})).unwrap()["verified"].as_bool().unwrap());
}

#[test]
fn apply_verifies_live_formulas_and_native_undo_restores_manual_cells() {
    let mut app = app();
    app.run("cell.set", json!({"cell":"A1","input":"Original"})).unwrap();
    app.run("ui.agent.remember", json!({})).unwrap();
    app.run("ui.agent.compare", json!({})).unwrap();
    let uid = app.session.doc().unwrap().uid;
    let result = app.run("ui.agent.apply", json!({})).unwrap();
    assert_eq!(result["verified"], true);
    assert_eq!(value(&app, "B5"), Value::number(15_000.0));
    assert_eq!(value(&app, "C5"), Value::number(15_500.0));
    assert_eq!(value(&app, "D5"), Value::number(500.0));
    assert!(app.session.doc().unwrap().wb.active().unwrap().cell(CellRef::parse("D5").unwrap()).unwrap().formula.is_some());
    assert_eq!(app.session.doc().unwrap().undo.len(), 2, "one whole summary is one native operation");
    assert_eq!(app.agent.receipts.len(), 1);
    assert_eq!(app.session.doc().unwrap().uid, uid);
    assert!(app.agent.preview.is_none(), "a second Apply requires a fresh preview");
    assert!(app.run("ui.agent.apply", json!({})).is_err());
    app.run("ui.agent.undo", json!({})).unwrap();
    assert_eq!(value(&app, "A1"), Value::text("Original"));
    assert!(value(&app, "C5").is_empty());
    assert_eq!(app.session.doc().unwrap().undo.len(), 1);
    assert_eq!(app.agent.receipts.len(), 2);
}

#[test]
fn unfinished_editor_draft_is_preserved_and_blocks_apply_and_undo() {
    let mut app = app();
    app.run("ui.agent.compare", json!({})).unwrap();
    app.editor = Some(EditState::new(0, CellRef::default(), "unfinished draft".into(), true, false));
    assert!(app.run("ui.agent.apply", json!({})).unwrap_err().contains("draft was kept"));
    assert!(app.run("ui.agent.undo", json!({})).is_err());
    assert_eq!(app.editor.as_ref().unwrap().text, "unfinished draft");
    assert!(value(&app, "A1").is_empty());
    assert!(app.agent.preview.is_some());
    assert!(app.session.doc().unwrap().undo.is_empty());
    assert!(app.session.ui_requests.is_empty());
}

#[test]
fn rendered_mode_toggle_and_suggestion_chip_drives_the_command_workflow() {
    use egui_kittest::kittest::Queryable;
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 1000.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app(),
    );
    harness.run_steps(5);
    let uid = harness.state().session.doc().unwrap().uid;
    harness.get_by_label("Agentic").click();
    harness.run_steps(2);
    assert!(harness.state().agent.enabled);
    harness.get_by_label("Remember to exclude test orders").click();
    harness.run_steps(2);
    harness.state_mut().run("ui.agent.submit", json!({"text":"Compare sales"})).unwrap();
    harness.run_steps(2);
    assert_eq!(harness.state().agent.preview.as_ref().unwrap().review.total().current_minor, 15_500);
    harness.get_by_label("Apply to workbook").click();
    harness.run_steps(2);
    assert_eq!(value(harness.state(), "C5"), Value::number(15_500.0));
    assert_eq!(harness.state().session.doc().unwrap().uid, uid);
    harness.get_by_label("Classic").click();
    harness.run_steps(2);
    assert!(!harness.state().agent.enabled);
    assert_eq!(harness.state().session.doc().unwrap().uid, uid);
}

mod connected {
    use gridcraft_engine::Session;
    use gridcraft_engine::core::{CellRef, Value};
    use gridcraft_ui_egui::agent::poll_at;
    use gridcraft_ui_egui::agent_api::{ApiRequest, Method, ScriptedApi};
    use gridcraft_ui_egui::agent_remote::{Connection, MAX_ATTEMPTS};
    use gridcraft_ui_egui::{Services, SheetApp, editor::EditState};
    use serde_json::{Value as Json, json};

    const RUN: &str = "0b7c2a3e-0000-4000-8000-000000000001";

    fn app() -> (SheetApp, ScriptedApi) {
        let api = ScriptedApi::default();
        api.route(Method::Get, "/api/status", 200, json!({"durable_agent_loop": true, "openai_configured": true, "database_configured": true}));
        api.route(
            Method::Get,
            "/api/files",
            200,
            json!([
                {"workbook_id": "11111111-1111-1111-1111-111111111111", "title": "Weekly Sales.xlsx", "origin": "mock", "latest_version": 1, "sha256": "aa"},
                {"workbook_id": "22222222-2222-2222-2222-222222222222", "title": "Quarterly Forecast.xlsx", "origin": "mock", "latest_version": 1, "sha256": "bb"},
                {"workbook_id": "44444444-4444-4444-4444-444444444444", "title": "Notes.pdf", "origin": "mock", "latest_version": 1, "sha256": "cc"}
            ]),
        );
        api.route(Method::Get, "/api/rules", 200, json!([]));
        let mut session = Session::new();
        session.new_workbook();
        let mut app = SheetApp::new(session, Services { agent_api: Some(Box::new(api.clone())), ..Services::default() });
        poll_at(&mut app, 0.0);
        poll_at(&mut app, 0.0);
        assert_eq!(app.agent.remote.connection, Connection::Connected);
        (app, api)
    }

    fn sheet(app: &SheetApp) -> String {
        app.session.doc().unwrap().wb.active().unwrap().name.clone()
    }

    fn value(app: &SheetApp, cell: &str) -> Value {
        app.session.doc().unwrap().wb.active().unwrap().value(CellRef::parse(cell).unwrap())
    }

    fn view(status: &str, pending: Json, actions: i32) -> Json {
        json!({"run_id": RUN, "status": status, "pending_tool": pending, "final_text": if status == "completed" { json!("Done.") } else { Json::Null },
               "action_count": actions, "sources": [{"kind": "sales_query", "label": "Synthetic demo sales", "id": "q-1", "version": 1}]})
    }

    fn write(sheet: &str) -> Json {
        json!({"tool_call_id": "call_1", "name": "write_range", "arguments": {}, "write": {"sheet": sheet, "range": "A1", "values": [["Total", 5]]}})
    }

    /// Submit, then answer the start request with a pending write proposal.
    fn propose(app: &mut SheetApp, api: &ScriptedApi) {
        app.run("ui.agent.submit", json!({"text": "Write the total"})).unwrap();
        let start = api.last().unwrap();
        assert_eq!(start.path, "/api/runs/start");
        let name = sheet(app);
        api.reply(start.ticket, 200, &view("awaiting_tool", write(&name), 0));
        poll_at(app, 0.0);
        assert!(app.agent.remote.approval.is_some());
        assert!(value(app, "A1").is_empty());
    }

    fn last_continue(api: &ScriptedApi) -> ApiRequest {
        let request = api.last().unwrap();
        assert_eq!(request.path, format!("/api/runs/{RUN}/continue"));
        request
    }

    fn outcome(request: &ApiRequest) -> Json {
        let body = request.body.clone().unwrap();
        assert_eq!(body["tool_call_id"], "call_1");
        body["outcome"].clone()
    }

    #[test]
    fn status_without_agent_loop_keeps_the_local_demo() {
        let api = ScriptedApi::default();
        api.route(Method::Get, "/api/status", 200, json!({"durable_agent_loop": false, "openai_configured": false, "database_configured": true}));
        let mut session = Session::new();
        session.new_workbook();
        let mut app = SheetApp::new(session, Services { agent_api: Some(Box::new(api.clone())), ..Services::default() });
        poll_at(&mut app, 0.0);
        poll_at(&mut app, 0.0);
        assert_eq!(app.agent.remote.connection, Connection::LocalDemo);
        app.run("ui.agent.submit", json!({"text": "Compare sales"})).unwrap();
        assert!(app.agent.preview.is_some(), "local demo still answers deterministically");
        assert_eq!(api.sent().len(), 1, "no run was started on the service");
    }

    #[test]
    fn manual_edit_after_a_proposal_makes_apply_report_stale_and_keeps_the_edit() {
        let (mut app, api) = app();
        propose(&mut app, &api);
        app.run("cell.set", json!({"cell": "A1", "input": "manual"})).unwrap();
        assert_eq!(app.run("ui.agent.apply", json!({})).unwrap()["outcome"], "stale_workbook");
        assert_eq!(outcome(&last_continue(&api)), json!({"ok": false, "result": null, "error": "stale_workbook"}));
        assert_eq!(value(&app, "A1"), Value::text("manual"));
        assert!(value(&app, "B1").is_empty());
    }

    #[test]
    fn busy_editor_reports_busy_and_keeps_the_draft() {
        let (mut app, api) = app();
        propose(&mut app, &api);
        app.editor = Some(EditState::new(0, CellRef::default(), "unfinished draft".into(), true, false));
        assert_eq!(app.run("ui.agent.apply", json!({})).unwrap()["outcome"], "busy");
        assert_eq!(outcome(&last_continue(&api))["error"], "busy");
        assert_eq!(app.editor.as_ref().unwrap().text, "unfinished draft");
        assert!(value(&app, "A1").is_empty());
    }

    #[test]
    fn discard_reports_user_declined_without_writing() {
        let (mut app, api) = app();
        propose(&mut app, &api);
        assert_eq!(app.run("ui.agent.discard", json!({})).unwrap()["outcome"], "user_declined");
        assert_eq!(outcome(&last_continue(&api))["error"], "user_declined");
        assert!(value(&app, "A1").is_empty());
        assert!(app.agent.remote.approval.is_none());
    }

    #[test]
    fn apply_writes_verifies_and_counts_completed_receipts() {
        let (mut app, api) = app();
        propose(&mut app, &api);
        assert_eq!(app.run("ui.agent.apply", json!({})).unwrap()["outcome"], "applied");
        assert_eq!(value(&app, "A1"), Value::text("Total"));
        assert_eq!(value(&app, "B1"), Value::Number(5.0));
        let request = last_continue(&api);
        let result = outcome(&request);
        assert_eq!(result["ok"], true);
        assert_eq!(result["result"]["verified"], true);
        assert_eq!(result["result"]["range"], "A1:B1");
        api.reply(request.ticket, 200, &view("completed", Json::Null, 1));
        poll_at(&mut app, 0.0);
        let run = app.agent.remote.run.clone().unwrap();
        assert_eq!(run.action_count, 1);
        assert_eq!(app.agent.remote.actions.len(), 1);
        assert_eq!(app.agent.messages.last().unwrap().text, "Done.");
    }

    #[test]
    fn network_failures_retry_the_identical_body_then_offer_retry() {
        let (mut app, api) = app();
        propose(&mut app, &api);
        app.run("ui.agent.apply", json!({})).unwrap();
        let first = last_continue(&api);
        let mut now = 0.0;
        let mut previous = first.clone();
        for attempt in 1..=MAX_ATTEMPTS {
            api.reply(previous.ticket, 0, &Json::Null);
            poll_at(&mut app, now);
            now += 10.0;
            poll_at(&mut app, now);
            if attempt < MAX_ATTEMPTS {
                let resent = last_continue(&api);
                assert_ne!(resent.ticket, previous.ticket);
                assert_eq!(resent.body, first.body, "same body and tool_call_id on retry {attempt}");
                previous = resent;
            }
        }
        assert!(app.agent.error.is_some());
        assert!(app.agent.remote.retry.is_some());
        let sent = api.sent().len();
        app.run("ui.agent.retry", json!({})).unwrap();
        assert_eq!(api.sent().len(), sent + 1);
        assert_eq!(last_continue(&api).body, first.body);
        assert_eq!(value(&app, "A1"), Value::text("Total"), "the write happened once");
    }

    #[test]
    fn files_search_attach_and_unsupported_formats() {
        let (mut app, api) = app();
        app.run("ui.agent.files", json!({"open": true})).unwrap();
        assert!(api.last().unwrap().path.starts_with("/api/files?q="));
        poll_at(&mut app, 0.0);
        assert_eq!(app.agent.remote.files.len(), 3);
        app.run("ui.agent.fileSearch", json!({"q": "weekly sales"})).unwrap();
        assert_eq!(api.last().unwrap().path, "/api/files?q=weekly%20sales");
        poll_at(&mut app, 0.0);
        assert_eq!(app.run("ui.agent.attach", json!({"workbook_id": "44444444-4444-4444-4444-444444444444"})).unwrap_err(), "Unsupported format");
        app.run("ui.agent.attach", json!({"workbook_id": "22222222-2222-2222-2222-222222222222"})).unwrap();
        app.run("ui.agent.fileOpen", json!({"workbook_id": "11111111-1111-1111-1111-111111111111"})).unwrap();
        assert_eq!(api.last().unwrap().path, "/api/workbooks/11111111-1111-1111-1111-111111111111/snapshots/1");
        app.run("ui.agent.submit", json!({"text": "Compare with the forecast"})).unwrap();
        let start = api.last().unwrap();
        assert_eq!(
            start.body.unwrap()["attachments"],
            json!([{"workbook_id": "22222222-2222-2222-2222-222222222222", "version": 1, "sha256": "bb"}])
        );
        assert!(app.agent.remote.attachments.is_empty());
    }

    fn connected_with_inbox(api: &ScriptedApi) -> (SheetApp, gridcraft_ui_egui::Inbox) {
        let inbox = gridcraft_ui_egui::Inbox::default();
        let mut session = Session::new();
        session.new_workbook();
        let services = Services { agent_api: Some(Box::new(api.clone())), inbox: Some(inbox.clone()), ..Services::default() };
        let mut app = SheetApp::new(session, services);
        poll_at(&mut app, 0.0);
        poll_at(&mut app, 0.0);
        (app, inbox)
    }

    fn open_from_inbox(app: &mut SheetApp, inbox: &gridcraft_ui_egui::Inbox) {
        let items: Vec<(String, Vec<u8>)> = std::mem::take(&mut *inbox.lock().unwrap());
        assert_eq!(items.len(), 1);
        for (name, bytes) in items {
            let b64 = gridcraft_engine::io::base64_encode(&bytes);
            app.run("file.open", json!({"name": name, "base64": b64})).unwrap();
        }
    }

    fn receipt(request: &ApiRequest, workbook: &str, version: i64) -> Json {
        let upload = request.upload.clone().unwrap();
        assert!(!upload.bytes.is_empty());
        let metadata: Json = serde_json::from_str(&upload.metadata).unwrap();
        json!({"workbook_id": workbook, "export_id": metadata["export_id"], "workbook_session_id": metadata["workbook_session_id"],
               "identity": metadata["identity"], "storage_version": version, "sha256": "00", "byte_length": upload.bytes.len()})
    }

    #[test]
    fn save_marks_saved_only_after_receipt_and_acknowledgement_and_keeps_later_edits() {
        let (mut app, api) = app();
        app.run("cell.set", json!({"cell": "A1", "input": "first"})).unwrap();
        app.run("ui.agent.save", json!({})).unwrap();
        let create = api.last().unwrap();
        assert_eq!(create.method, Method::Put);
        let workbook = create.path.trim_start_matches("/api/workbooks/").to_string();
        api.reply(create.ticket, 200, &json!({"workbook_id": workbook, "title": "Book1.xlsx", "latest_version": 0}));
        poll_at(&mut app, 0.0);
        let upload = api.last().unwrap();
        assert_eq!(upload.path, format!("/api/workbooks/{workbook}/snapshots"));
        let metadata: Json = serde_json::from_str(&upload.upload.clone().unwrap().metadata).unwrap();
        assert_eq!(metadata["expected_version"], 0);
        app.run("cell.set", json!({"cell": "A2", "input": "later"})).unwrap();
        assert!(app.session.doc().unwrap().is_dirty(), "nothing is saved before the receipt");
        api.reply(upload.ticket, 200, &receipt(&upload, &workbook, 1));
        poll_at(&mut app, 0.0);
        let doc = app.session.doc().unwrap();
        assert!(doc.is_dirty(), "the later edit stays unsaved");
        assert_eq!(doc.saved.sheets[0].value(CellRef::parse("A1").unwrap()), Value::text("first"));
        assert!(doc.saved.sheets[0].value(CellRef::parse("A2").unwrap()).is_empty());
        let stored: Json = serde_json::from_str(api.log.borrow().storage.get("gridcraft.agent.workbook").unwrap()).unwrap();
        assert_eq!(stored["version"], 1);
        app.run("ui.agent.save", json!({})).unwrap();
        let second = api.last().unwrap();
        let metadata: Json = serde_json::from_str(&second.upload.unwrap().metadata).unwrap();
        assert_eq!(metadata["expected_version"], 1, "the next save expects the committed version");
    }

    #[test]
    fn save_retries_the_same_export_and_a_conflict_is_not_saved() {
        let (mut app, api) = app();
        app.run("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
        app.run("ui.agent.save", json!({})).unwrap();
        let create = api.last().unwrap();
        api.reply(create.ticket, 200, &json!({}));
        poll_at(&mut app, 0.0);
        let first = api.last().unwrap();
        api.reply(first.ticket, 0, &Json::Null);
        poll_at(&mut app, 0.0);
        poll_at(&mut app, 10.0);
        let retry = api.last().unwrap();
        assert_ne!(retry.ticket, first.ticket);
        assert_eq!(retry.upload, first.upload, "identical export ID, metadata and bytes");
        api.reply(retry.ticket, 409, &json!({"error": "version_conflict"}));
        poll_at(&mut app, 10.0);
        assert!(app.session.doc().unwrap().is_dirty());
        assert!(app.agent.remote.save_error.as_ref().unwrap().contains("version conflict"));
        assert!(app.agent.remote.save.is_none());
    }

    #[test]
    fn reload_recovers_the_latest_committed_workbook_then_the_run() {
        let api = ScriptedApi::default();
        api.route(Method::Get, "/api/status", 200, json!({"durable_agent_loop": true, "openai_configured": true, "database_configured": true}));
        let workbook = "11111111-1111-1111-1111-111111111111";
        api.log
            .borrow_mut()
            .storage
            .insert("gridcraft.agent.workbook".into(), json!({"workbook_id": workbook, "version": 1, "title": "Weekly Sales.xlsx"}).to_string());
        api.log.borrow_mut().storage.insert("gridcraft.agent.run".into(), RUN.into());
        let (mut app, inbox) = connected_with_inbox(&api);
        let latest = api.last().unwrap();
        assert_eq!(latest.path, format!("/api/workbooks/{workbook}/snapshots/latest"));
        let bytes = gridcraft_engine::demo_files::weekly_sales_xlsx().unwrap();
        api.reply_raw(latest.ticket, 200, bytes, &[("x-gridcraft-storage-version", "3")]);
        poll_at(&mut app, 0.0);
        assert!(app.agent.remote.pending_open.is_some());
        assert!(app.run("ui.agent.submit", json!({"text": "Compare sales"})).is_err(), "agent waits for the correlated open");
        open_from_inbox(&mut app, &inbox);
        poll_at(&mut app, 0.0);
        assert!(app.agent.remote.pending_open.is_none());
        let cloud = app.agent.remote.cloud.clone().unwrap();
        assert_eq!((cloud.workbook_id.as_str(), cloud.version), (workbook, 3));
        assert_eq!(cloud.document_id, Some(app.session.doc().unwrap().uid));
        let run = api.last().unwrap();
        assert_eq!((run.method, run.path.as_str()), (Method::Get, format!("/api/runs/{RUN}").as_str()));
        api.reply(run.ticket, 200, &json!({"run_id": RUN, "status": "interrupted", "pending_tool": null, "final_text": "Run stopped: provider_timeout", "action_count": 0, "sources": []}));
        poll_at(&mut app, 0.0);
        assert_eq!(app.agent.remote.retry.as_ref().unwrap().path, format!("/api/runs/{RUN}/resume"));
    }
}
