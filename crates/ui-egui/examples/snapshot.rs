//! Renders the whole GridCraft window offscreen (wgpu, no window) to a PNG.
//!
//! `cargo run --release -p gridcraft-ui-egui --example snapshot -- [--sample sales] [--in file.xlsx]
//!  [--size 1440x900] [--scale 2] [--cmd 'id={json}']... [--tab Insert] [--dark] [--language en|zh|ja|ko|ru|pt|cs]
//!  [--dialog 'name={json}'] [--hf-edit header|footer] [--agent-fixture] out.png`
//!
//! `--agent-fixture` connects the Agentic workspace to a scripted in-memory service (status,
//! three mock files, rules and one completed run with sources) so connected states render offline.

use gridcraft_engine::Session;
use gridcraft_ui_egui::agent_api::{Method, ScriptedApi};
use gridcraft_ui_egui::{Services, SheetApp};
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut sample = None;
    let mut input = None;
    let mut size = (1440.0f32, 900.0f32);
    let mut scale = 2.0f32;
    let mut cmds: Vec<String> = vec![];
    let mut tab = None;
    let mut dark = false;
    let mut pane: Option<String> = None;
    let mut chart: Option<u32> = None;
    let mut dialog: Option<String> = None;
    let mut language: Option<String> = None;
    let mut hf_edit: Option<String> = None;
    let mut agent_fixture = false;
    let mut out = "snapshot.png".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--sample" => {
                i += 1;
                sample = args.get(i).cloned();
            }
            "--in" => {
                i += 1;
                input = args.get(i).cloned();
            }
            "--size" => {
                i += 1;
                if let Some((w, h)) = args.get(i).and_then(|s| s.split_once('x')) {
                    size = (w.parse().unwrap_or(1440.0), h.parse().unwrap_or(900.0));
                }
            }
            "--scale" => {
                i += 1;
                scale = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(2.0);
            }
            "--cmd" => {
                i += 1;
                if let Some(c) = args.get(i) {
                    cmds.push(c.clone());
                }
            }
            "--tab" => {
                i += 1;
                tab = args.get(i).cloned();
            }
            "--dark" => dark = true,
            "--agent-fixture" => agent_fixture = true,
            "--pane" => {
                i += 1;
                pane = args.get(i).cloned();
            }
            "--chart" => {
                i += 1;
                chart = args.get(i).and_then(|s| s.parse().ok());
            }
            "--dialog" => {
                i += 1;
                dialog = args.get(i).cloned();
            }
            "--hf-edit" => {
                i += 1;
                hf_edit = args.get(i).cloned();
            }
            "--language" => {
                i += 1;
                language = args.get(i).cloned();
            }
            other => out = other.to_string(),
        }
        i += 1;
    }
    let mut session = Session::new();
    if let Some(p) = &input {
        session.execute("file.open", json!({"path": p})).expect("open");
    } else if let Some(s) = &sample {
        session.execute("file.new", json!({"sample": s})).expect("sample");
    } else if let Ok(bytes) = gridcraft_engine::demo_files::weekly_sales_xlsx() {
        let encoded = gridcraft_engine::io::base64_encode(&bytes);
        let _ = session.execute("file.open", json!({"name": "Weekly Sales.xlsx", "base64": encoded}));
    } else {
        session.new_workbook();
    }
    let mut harness =
        egui_kittest::Harness::builder().with_size(egui::vec2(size.0, size.1)).with_pixels_per_point(scale).with_max_steps(12).wgpu().build_ui_state(
            move |ui, app: &mut SheetApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            {
                let services =
                    if agent_fixture { Services { agent_api: Some(Box::new(fixture())), ..Services::default() } } else { Services::default() };
                let mut app = SheetApp::new(session, services);
                gridcraft_ui_egui::agent::poll_at(&mut app, 0.0);
                gridcraft_ui_egui::agent::poll_at(&mut app, 0.0);
                // UI commands (including the agent workspace) share the same command path.
                for c in &cmds {
                    let (id, p) = c
                        .split_once('=')
                        .map(|(a, b)| (a.to_string(), serde_json::from_str(b).unwrap_or(json!({}))))
                        .unwrap_or((c.clone(), json!({})));
                    if let Err(e) = app.run(&id, p) {
                        eprintln!("{id}: {e}");
                    }
                    gridcraft_ui_egui::agent::poll_at(&mut app, 0.0);
                }
                app.ui.dark = dark;
                if let Some(code) = &language
                    && let Some(l) = gridcraft_ui_egui::i18n::Language::parse(code)
                {
                    app.ui.language = l;
                }
                if let Some(t) = tab {
                    app.ui.ribbon_tab = t;
                }
                app.grid.pane = pane;
                app.selected_chart = chart;
                if let Some(d) = &dialog {
                    let (name, p) =
                        d.split_once('=').map(|(a, b)| (a, serde_json::from_str(b).unwrap_or(json!({})))).unwrap_or((d.as_str(), json!({})));
                    app.open_dialog(name, p);
                }
                // Page Layout view: edit the first page's centre header (or footer) section.
                let active = app.session.active().and_then(|d| d.wb.active().cloned().map(|s| (d.wb.active_sheet, s)));
                if let (Some(part), Some((si, sh))) = (&hf_edit, active) {
                    let zone = gridcraft_engine::paper::HfZone { footer: part == "footer", section: 1 };
                    gridcraft_ui_egui::paper_view::begin(&mut app, si, &sh, (0, 0), zone);
                }
                app
            },
        );
    harness.run_steps(8);
    let img = harness.render().expect("render");
    img.save(&out).expect("save png");
    eprintln!("wrote {out} ({}x{})", img.width(), img.height());
}

fn fixture() -> ScriptedApi {
    let api = ScriptedApi::default();
    api.route(Method::Get, "/api/status", 200, json!({"durable_agent_loop": true, "openai_configured": true, "database_configured": true}));
    api.route(
        Method::Get,
        "/api/files/",
        200,
        json!({"workbook_id": "22222222-2222-2222-2222-222222222222", "version": 1, "sheets": [{"name": "Forecast", "cells": [
            ["Quarter", "Forecast (EUR)", "Owner"], ["Q1", 1200, "Mock team"], ["Q2", 1350, "Mock team"], ["Q3", 1500, "Mock team"]]}]}),
    );
    api.route(
        Method::Get,
        "/api/files",
        200,
        json!([
            {"workbook_id": "11111111-1111-1111-1111-111111111111", "title": "Weekly Sales.xlsx", "origin": "mock", "latest_version": 1, "sha256": "11"},
            {"workbook_id": "22222222-2222-2222-2222-222222222222", "title": "Quarterly Forecast.xlsx", "origin": "mock", "latest_version": 1, "sha256": "22"},
            {"workbook_id": "33333333-3333-3333-3333-333333333333", "title": "Inventory.xlsx", "origin": "mock", "latest_version": 1, "sha256": "33"}
        ]),
    );
    api.route(
        Method::Get,
        "/api/rules",
        200,
        json!([{"id": "exclude-tests-v1", "original_wording": "Remember to exclude test orders", "kind": "exclude_test_orders", "enabled": true}]),
    );
    api.route(
        Method::Post,
        "/api/runs/start",
        200,
        json!({"run_id": "0b7c2a3e-0000-4000-8000-000000000001", "status": "completed", "pending_tool": null, "action_count": 1,
               "final_text": "Total net revenue was EUR 150.00 for 5-12 Oct and EUR 155.00 for 12-19 Oct 2026 UTC, excluding test orders. The Review sheet was updated and verified at A1:E9.",
               "sources": [
                   {"kind": "sales_query", "label": "Synthetic demo sales | revision 1 | paid gross minus refunds", "id": "6f1d3c2a-5b7e-4f00-9a11-000000000155", "version": 1},
                   {"kind": "rule", "label": "Remember to exclude test orders", "id": "exclude-tests-v1", "version": null}
               ]}),
    );
    api
}
