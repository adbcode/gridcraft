//! Embedding hosts acknowledge the exact UI export, even when the user edits during upload.
use std::cell::RefCell;
use std::rc::Rc;

use gridcraft_engine::{
    Session,
    agent_bridge::BridgeState,
    core::{CellRef, Value},
};
use gridcraft_ui_egui::{Services, SheetApp};
use serde_json::json;

#[test]
fn ui_save_captures_exported_document_and_late_ack_keeps_edits_dirty() {
    let adapter = Rc::new(RefCell::new(BridgeState::default()));
    let saved_bytes = Rc::new(RefCell::new(Vec::new()));
    let capture = adapter.clone();
    let bytes = saved_bytes.clone();
    let services = Services {
        download: Some(Box::new(|_, _| panic!("XLSX must use the document-aware callback"))),
        download_workbook: Some(Box::new(move |document, _, exported| {
            capture.borrow_mut().capture_document_snapshot(document, "ui-save-1").map_err(|e| e.to_string())?;
            *bytes.borrow_mut() = exported.to_vec();
            Ok(())
        })),
        ..Default::default()
    };
    let mut session = Session::new();
    session.new_workbook();
    session.execute("cell.set", json!({"cell":"A1", "input":"42"})).unwrap();
    let mut app = SheetApp::new(session, services);
    app.open_dialog("saveAs", json!({}));
    assert!(app.session.active().unwrap().is_dirty(), "posting bytes is not persistence");
    let (exported, _) = gridcraft_engine::io::open_bytes("Book.xlsx", &saved_bytes.borrow()).unwrap();
    assert_eq!(exported.active().unwrap().cell(CellRef::new(0, 0)).unwrap().value, Value::Number(42.0));
    app.run("cell.set", json!({"cell":"A1", "input":"99"})).unwrap();
    let ack = adapter.borrow_mut().acknowledge_snapshot(&mut app.session, "ui-save-1").unwrap();
    assert!(!ack.clean);
    assert!(app.session.active().unwrap().is_dirty());
    assert_eq!(app.session.active().unwrap().wb.active().unwrap().cell(CellRef::new(0, 0)).unwrap().value, Value::Number(99.0));
}

#[test]
fn non_workbook_export_uses_download_and_an_editor_is_busy_without_committing_it() {
    let downloads = Rc::new(RefCell::new(0));
    let record = downloads.clone();
    let mut session = Session::new();
    session.new_workbook();
    let mut app = SheetApp::new(
        session,
        Services {
            download: Some(Box::new(move |_, _| *record.borrow_mut() += 1)),
            download_workbook: Some(Box::new(|_, _, _| panic!("CSV is not a workbook snapshot"))),
            ..Default::default()
        },
    );
    app.deliver_download("review.csv", b"42").unwrap();
    assert_eq!(*downloads.borrow(), 1);
    assert!(!app.host_busy());
    app.grid.renaming_tab = Some(0);
    assert!(app.host_busy());
    app.grid.renaming_tab = None;
    app.grid.drag = gridcraft_ui_egui::grid::Drag::Fill { target: None };
    assert!(app.host_busy());
    app.grid.drag = gridcraft_ui_egui::grid::Drag::None;
    app.editor = Some(gridcraft_ui_egui::editor::EditState::new(0, CellRef::new(0, 0), "unsubmitted".into(), true, false));
    assert!(app.host_busy());
    assert!(app.editor.is_some());
    assert!(!app.session.active().unwrap().is_dirty());
}
