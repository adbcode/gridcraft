//! Provider-free first slice: export a verified synthetic review, then undo its live edit.
use std::error::Error;
use std::io::Write;

use gridcraft_engine::{
    Session,
    agent_bridge::BridgeState,
    review_workflow::{ComparisonPeriods, compare_sales, demo_rules, demo_sales},
};
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args().nth(1).ok_or("usage: cargo run -p gridcraft-engine --example agentic_review -- /tmp/review.xlsx (new file)")?;
    let review = compare_sales(&demo_sales(), &demo_rules()?, "demo-sales-v1", ComparisonPeriods::demo())?;
    let values = review.sheet_values();
    let range = format!("A1:E{}", values.len());
    let mut session = Session::new();
    session.new_workbook();
    let mut bridge = BridgeState::default();
    let identity = bridge.observe(&session)?.identity;
    bridge.handle(&mut session, "inspect", json!({"expected":identity, "range":range}))?;
    let request = json!({"request_id":"demo-write", "expected":identity, "range":range, "values":values});
    let written = bridge.handle(&mut session, "set_values", request.clone())?;
    for (cell, value) in [("D3", -4000.0), ("E3", -0.4), ("D5", 500.0)] {
        let actual = written
            .get("readback")
            .and_then(|r| r.get("cells"))
            .and_then(Value::as_array)
            .and_then(|cells| cells.iter().find(|entry| entry.get("cell").and_then(Value::as_str) == Some(cell)))
            .and_then(|entry| entry.get("value"))
            .and_then(Value::as_f64);
        if actual != Some(value) {
            return Err(format!("verification failed at {cell}: {actual:?}").into());
        }
    }
    if bridge.handle(&mut session, "set_values", request)? != written {
        return Err("duplicate write did not replay its receipt".into());
    }
    let identity = bridge.observe(&session)?.identity;
    let snapshot = bridge.capture_snapshot(&session, "demo-export", &identity)?;
    let bytes = gridcraft_engine::io::save_bytes(&snapshot.workbook, "Review.xlsx")?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    // Acknowledges this local file write only; there is no database adapter in this example.
    bridge.acknowledge_snapshot(&mut session, "demo-export")?;
    let identity = bridge.observe(&session)?.identity;
    let undone = bridge.handle(&mut session, "undo", json!({"request_id":"demo-undo", "expected":identity, "range":range}))?;
    let cells = undone.get("readback").and_then(|r| r.get("cells")).and_then(Value::as_array).ok_or("undo readback missing")?;
    if cells.iter().any(|cell| cell.get("value").is_none_or(|v| !v.is_null())) {
        return Err("undo did not restore the blank sheet".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode":"offline synthetic data", "workbook":path, "provenance":review.provenance(),
            "verified":{"jackets_delta_minor":-4000,"jackets_growth":-0.4,"total_delta_minor":500},
            "duplicate_write":"replayed", "live_undo":"verified", "persistence":"local file only"
        }))?
    );
    Ok(())
}
