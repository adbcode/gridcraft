//! Durable run loop against a real disposable PostgreSQL database and a scripted local
//! stand-in for the chat-completions API. No external provider is contacted.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Method, Request, StatusCode, header},
    routing::post,
};
use gridcraft_server::{
    providers::{OpenAiClient, OpenAiConfig},
    runs::RunService,
    storage::{Store, migrate},
    web::{AppState, router},
};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn call(counter: &AtomicUsize, name: &str, arguments: Value) -> Value {
    let id = format!("call_{}", counter.fetch_add(1, Ordering::SeqCst));
    json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
        "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}}]})
}

fn answer(text: &str) -> Value {
    json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":text}}]})
}

/// Decides the next turn from the replayed conversation, like a model would.
async fn fake_completion(State(counter): State<Arc<AtomicUsize>>, Json(request): Json<Value>) -> Json<Value> {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let last_user = messages.iter().rposition(|m| m["role"] == "user").unwrap_or_default();
    let prompt = messages.get(last_user).and_then(|m| m["content"].as_str()).unwrap_or_default().to_owned();
    let tools: Vec<Value> = messages
        .iter()
        .skip(last_user)
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["content"].as_str().and_then(|c| serde_json::from_str(c).ok()))
        .collect();
    let reply = if prompt.starts_with("Remember") {
        if tools.is_empty() {
            call(&counter, "remember_rule", json!({"instruction":"Remember to exclude test orders"}))
        } else {
            answer("Saved the rule.")
        }
    } else {
        match tools.last() {
            None => call(&counter, "compare_sales", json!({})),
            Some(last) if last.pointer("/result/query_id").is_some() => {
                call(&counter, "write_sales_review", json!({"query_id": last.pointer("/result/query_id").cloned().unwrap_or_default()}))
            }
            Some(_) => answer("Wrote the sales review."),
        }
    };
    Json(reply)
}

async fn request(app: &Router, method: Method, path: &str, body: Option<Value>) -> TestResult<(StatusCode, Value)> {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost:8080")
        .header(header::ORIGIN, "http://localhost:8080")
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024).await?;
    Ok((status, serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
}

fn start(request_id: &str, text: &str) -> Value {
    json!({"request_id":request_id,"workbook_session_id":"session-1","text":text,"attachments":[],
        "context":{"identity":{"document_id":1,"sheet_index":0,"sheet_name":"Review","change_token":3},
            "selection":"A1","active_cell":"A1","dirty":false,"undo_available":false,"ready":true,"calculation_mode":"Automatic"}})
}

async fn compare_total(pool: &sqlx::PgPool, run_id: &str) -> TestResult<String> {
    let result: Value = sqlx::query_scalar("SELECT result FROM agent_tool_receipts WHERE run_id=$1::uuid AND tool_name='compare_sales'")
        .bind(run_id)
        .fetch_one(pool)
        .await?;
    Ok(result.pointer("/result/total/current_eur").and_then(Value::as_str).unwrap_or_default().to_owned())
}

#[tokio::test]
#[ignore = "requires an explicit disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn durable_runs_are_idempotent_source_bound_and_recoverable() -> TestResult {
    let url = std::env::var("GRIDCRAFT_TEST_DATABASE_URL")?;
    let pool = PgPoolOptions::new().max_connections(5).connect(&url).await?;
    migrate(&pool).await?;
    let store = Store::new(pool.clone(), Uuid::new_v4());
    store.seed_demo().await?;

    let fake = Router::new().route("/v1/chat/completions", post(fake_completion)).with_state(Arc::new(AtomicUsize::new(0)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move { axum::serve(listener, fake).await });
    let client = OpenAiClient::new(OpenAiConfig {
        api_key: "test-key".into(),
        base_url: format!("http://127.0.0.1:{port}/v1"),
        model: "scripted".into(),
        reasoning_effort: None,
    })?;
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("index.html"), "test web asset")?;
    let app = router(AppState {
        store: Some(store.clone()),
        static_dir: directory.path().into(),
        openai_configured: true,
        speech_configured: false,
        tts_configured: false,
        runs: Some(RunService::new(store, Some(client))),
        speech: None,
    });

    let (status, run) = request(&app, Method::POST, "/api/runs/start", Some(start("r1", "Compare sales"))).await?;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["status"], "awaiting_tool");
    assert_eq!(run["pending_tool"]["name"], "write_sales_review");
    assert_eq!(run["pending_tool"]["write"]["sheet"], "Review");
    assert!(run["pending_tool"]["write"]["values"].as_array().is_some_and(|rows| rows.len() > 3));
    assert_eq!(run["sources"][0]["kind"], "sales_query");
    let run_id = run["run_id"].as_str().unwrap_or_default().to_owned();
    assert_eq!(compare_total(&pool, &run_id).await?, "655.00");
    let tool_call_id = run["pending_tool"]["tool_call_id"].clone();

    let (status, replay) = request(&app, Method::POST, "/api/runs/start", Some(start("r1", "Compare sales"))).await?;
    assert_eq!((status, replay["run_id"].clone()), (StatusCode::OK, json!(run_id)));
    let (status, _) = request(&app, Method::POST, "/api/runs/start", Some(start("r1", "Something else"))).await?;
    assert_eq!(status, StatusCode::CONFLICT);

    let path = format!("/api/runs/{run_id}/continue");
    let (status, _) = request(&app, Method::POST, &path, Some(json!({"tool_call_id":"call_wrong","outcome":{"ok":true}}))).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let ok = json!({"tool_call_id":tool_call_id,"outcome":{"ok":true,"result":{"cells_written":30}}});
    let (status, done) = request(&app, Method::POST, &path, Some(ok.clone())).await?;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!((done["status"].clone(), done["action_count"].clone()), (json!("completed"), json!(1)));
    assert_eq!(done["final_text"], "Wrote the sales review.");
    let (status, retried) = request(&app, Method::POST, &path, Some(ok)).await?;
    assert_eq!((status, retried), (StatusCode::OK, done.clone()));
    let different = json!({"tool_call_id":tool_call_id,"outcome":{"ok":false,"error":"user_declined"}});
    assert_eq!(request(&app, Method::POST, &path, Some(different)).await?.0, StatusCode::CONFLICT);

    let (status, remembered) = request(&app, Method::POST, "/api/runs/start", Some(start("r2", "Remember to exclude test orders"))).await?;
    assert_eq!(status, StatusCode::OK, "{remembered}");
    assert_eq!((remembered["status"].clone(), remembered["action_count"].clone()), (json!("completed"), json!(1)));
    assert!(remembered["sources"].as_array().is_some_and(|sources| sources.iter().any(|s| s["kind"] == "rule")));

    let (_, second) = request(&app, Method::POST, "/api/runs/start", Some(start("r3", "Compare sales"))).await?;
    let second_id = second["run_id"].as_str().unwrap_or_default().to_owned();
    assert_eq!(compare_total(&pool, &second_id).await?, "155.00");
    let declined = json!({"tool_call_id":second["pending_tool"]["tool_call_id"],"outcome":{"ok":false,"error":"user_declined"}});
    let (_, declined) = request(&app, Method::POST, &format!("/api/runs/{second_id}/continue"), Some(declined)).await?;
    assert_eq!((declined["status"].clone(), declined["action_count"].clone()), (json!("completed"), json!(0)));

    sqlx::query("UPDATE agent_runs SET status='running',final_text=NULL,updated_at=now()-interval '5 minutes' WHERE run_id=$1::uuid")
        .bind(&second_id)
        .execute(&pool)
        .await?;
    let (_, stale) = request(&app, Method::GET, &format!("/api/runs/{second_id}"), None).await?;
    assert_eq!(stale["status"], "interrupted");
    let (status, resumed) = request(&app, Method::POST, &format!("/api/runs/{second_id}/resume"), None).await?;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["status"], "completed");
    assert_eq!(request(&app, Method::POST, &format!("/api/runs/{second_id}/resume"), None).await?.0, StatusCode::CONFLICT);

    let (status, files) = request(&app, Method::GET, "/api/files?q=", None).await?;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = files.as_array().map(|f| f.iter().filter_map(|f| f["title"].as_str()).collect()).unwrap_or_default();
    assert_eq!(titles, ["Weekly Sales.xlsx", "Quarterly Forecast.xlsx", "Inventory.xlsx"]);
    assert!(files.as_array().is_some_and(|f| f.iter().all(|f| f["origin"] == "mock" && f["latest_version"] == 1)));
    let id = files[1]["workbook_id"].as_str().unwrap_or_default();
    let (status, preview) = request(&app, Method::GET, &format!("/api/files/{id}/versions/1/preview"), None).await?;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(preview["sheets"][0]["cells"].as_array().is_some_and(|rows| rows.len() <= 20));
    Ok(())
}
