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
fn rendered_mode_toggle_and_demo_buttons_drive_the_command_workflow() {
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
    harness.get_by_label("Remember: exclude test orders").click();
    harness.run_steps(2);
    harness.get_by_label("Compare demo sales").click();
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
