//! HTTP handlers backed by a real disposable PostgreSQL database. No network listener,
//! browser, Aiven or model provider is used. Generated XLSX bytes come from the live engine.

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use gridcraft_engine::{
    Session,
    agent_bridge::BridgeState,
    review_workflow::{ComparisonPeriods, demo_sales},
};
use gridcraft_server::{
    storage::{MAX_SNAPSHOT_BYTES, StorageError, Store, migrate},
    web::{AppState, router},
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn json_request(app: &Router, method: Method, path: &str, value: Value) -> TestResult<(StatusCode, Value)> {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost:8080")
        .header(header::ORIGIN, "http://localhost:8080")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&value)?))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024).await?;
    let value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) if !status.is_success() => json!({"raw_error":String::from_utf8_lossy(&bytes)}),
        Err(error) => return Err(error.into()),
    };
    Ok((status, value))
}

async fn get(app: &Router, path: &str) -> TestResult<Response> {
    Ok(app.clone().oneshot(Request::builder().uri(path).body(Body::empty())?).await?)
}

async fn snapshot_request(app: &Router, workbook_id: Uuid, metadata_json: &str, file: &[u8]) -> TestResult<Response> {
    const BOUNDARY: &str = "gridcraft-service-test-boundary";
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata_json}\r\n\
         --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"review.xlsx\"\r\n\
         Content-Type: application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/workbooks/{workbook_id}/snapshots"))
        .header(header::HOST, "localhost:8080")
        .header(header::ORIGIN, "http://localhost:8080")
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body))?;
    Ok(app.clone().oneshot(request).await?)
}

async fn cleanup(pool: &PgPool, workspace_id: Uuid) -> Result<(), StorageError> {
    for statement in [
        "DELETE FROM review_snapshots WHERE workspace_id=$1",
        "DELETE FROM review_workbooks WHERE workspace_id=$1",
        "DELETE FROM review_queries WHERE workspace_id=$1",
        "DELETE FROM review_rules WHERE workspace_id=$1",
        "DELETE FROM review_sales WHERE workspace_id=$1",
        "DELETE FROM review_workspaces WHERE workspace_id=$1",
    ] {
        sqlx::query(statement).bind(workspace_id).execute(pool).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires an explicit disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn service_persists_review_evidence_and_acknowledges_exact_workbook_uploads() -> TestResult {
    let url = std::env::var("GRIDCRAFT_TEST_DATABASE_URL")
        .map_err(|_| "GRIDCRAFT_TEST_DATABASE_URL must explicitly identify a disposable PostgreSQL database")?;
    let pool = PgPoolOptions::new().max_connections(5).connect(&url).await.map_err(|_| StorageError::Database)?;
    migrate(&pool).await?;
    let workspace = Uuid::new_v4();
    let store = Store::new(pool.clone(), workspace);
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("index.html"), "test web asset")?;
    let app = router(AppState {
        store: Some(store),
        static_dir: directory.path().into(),
        openai_configured: false,
        speech_configured: false,
        tts_configured: false,
        openai: None,
        elevenlabs: None,
    });
    let ready = get(&app, "/api/health/ready").await?;
    assert_eq!(ready.status(), StatusCode::OK);
    let readiness: Value = serde_json::from_slice(&to_bytes(ready.into_body(), 4096).await?)?;
    assert_eq!(readiness.get("providers"), Some(&json!("not_probed")));

    let (status, imported) = json_request(&app, Method::POST, "/api/sales/import", json!({"records":demo_sales()})).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(imported.pointer("/result/source_revision"), Some(&json!(1)));
    let (status, rule) =
        json_request(&app, Method::POST, "/api/rules", json!({"id":"exclude-test-orders","instruction":"Remember to exclude test orders"})).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rule.get("kind"), Some(&json!("exclude_test_orders")));
    let (status, _) = json_request(
        &app,
        Method::POST,
        "/api/rules",
        json!({"id":"owner-override","instruction":"exclude test orders","workspace_id":Uuid::new_v4()}),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, comparison) = json_request(&app, Method::POST, "/api/sales/compare", json!({"periods":ComparisonPeriods::demo()})).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(comparison.pointer("/review/total/previous_minor"), Some(&json!(15_000)));
    assert_eq!(comparison.pointer("/review/total/current_minor"), Some(&json!(15_500)));
    let evidence_url = comparison.get("evidence_url").and_then(Value::as_str).ok_or("comparison did not provide evidence URL")?;
    let evidence_response = get(&app, evidence_url).await?;
    assert_eq!(evidence_response.status(), StatusCode::OK);
    let evidence: Value = serde_json::from_slice(&to_bytes(evidence_response.into_body(), 1024 * 1024).await?)?;
    assert_eq!(evidence.get("source_revision"), Some(&json!(1)));
    assert_eq!(evidence.get("result"), comparison.get("review"));
    assert_eq!(evidence.get("source_records").and_then(Value::as_array).map(Vec::len), Some(12));
    assert_eq!(evidence.pointer("/rule_context/0/id"), Some(&json!("exclude-test-orders")));

    let workbook_id = Uuid::new_v4();
    let (status, _) = json_request(&app, Method::PUT, &format!("/api/workbooks/{workbook_id}"), json!({"title":"Weekly review"})).await?;
    assert_eq!(status, StatusCode::OK);

    // Export an actual workbook from the live engine and carry its exact identity to
    // PostgreSQL. A later edit must remain dirty when that older upload is acknowledged.
    let mut session = Session::new();
    session.new_workbook();
    let mut bridge = BridgeState::default();
    let identity = bridge.observe(&session)?.identity;
    let values = comparison.get("sheet_values").cloned().ok_or("missing editable summary")?;
    let row_count = values.as_array().ok_or("summary must be rectangular")?.len();
    bridge.handle(
        &mut session,
        "set_values",
        json!({"request_id":"write-review","expected":identity,"range":format!("A1:E{row_count}"),"values":values}),
    )?;
    let identity = bridge.observe(&session)?.identity;
    let captured = bridge.capture_snapshot(&session, "export-1", &identity)?;
    let bytes = gridcraft_engine::io::save_bytes(&captured.workbook, "review.xlsx")?;
    let metadata = serde_json::to_string(&json!({
        "export_id":"export-1","workbook_session_id":"browser-session-1","identity":captured.identity,"expected_version":0,
    }))?;
    session.execute("cell.set", json!({"cell":"H1","input":"later unsaved edit"}))?;

    let response = snapshot_request(&app, workbook_id, &metadata, &bytes).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let receipt: Value = serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await?)?;
    assert_eq!(receipt.get("storage_version"), Some(&json!(1)));
    assert_eq!(receipt.get("identity"), Some(&serde_json::to_value(&captured.identity)?));
    let acknowledgement = bridge.acknowledge_snapshot(&mut session, "export-1")?;
    assert!(!acknowledgement.clean);
    assert!(session.active().ok_or("missing live workbook")?.is_dirty());

    let repeated = snapshot_request(&app, workbook_id, &metadata, &bytes).await?;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(serde_json::from_slice::<Value>(&to_bytes(repeated.into_body(), 16_384).await?)?, receipt);
    let changed = snapshot_request(&app, workbook_id, &metadata, b"different bytes").await?;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    let malformed = snapshot_request(&app, workbook_id, "{not valid JSON}", &bytes).await?;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    let oversized = snapshot_request(&app, workbook_id, &metadata, &vec![0; MAX_SNAPSHOT_BYTES.saturating_add(1)]).await?;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let download = get(&app, &format!("/api/workbooks/{workbook_id}/snapshots/latest")).await?;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers().get("x-gridcraft-storage-version").and_then(|value| value.to_str().ok()), Some("1"));
    assert_eq!(download.headers().get("x-gridcraft-sha256").and_then(|value| value.to_str().ok()), receipt.get("sha256").and_then(Value::as_str));
    assert_eq!(download.headers().get(header::CACHE_CONTROL).and_then(|value| value.to_str().ok()), Some("no-store"));
    let restored = to_bytes(download.into_body(), MAX_SNAPSHOT_BYTES).await?;
    assert_eq!(restored.as_ref(), bytes.as_slice());
    gridcraft_engine::io::open_bytes("restored.xlsx", &restored)?;

    cleanup(&pool, workspace).await?;
    drop(app);
    pool.close().await;
    Ok(())
}
