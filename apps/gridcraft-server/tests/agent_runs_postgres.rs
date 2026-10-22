//! Durable workflow acceptance with real PostgreSQL, live engine and a local HTTP model mock.
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use gridcraft_engine::{
    Session,
    agent_bridge::BridgeState,
    review_workflow::ComparisonPeriods,
    workspace::{StartRun, ToolContinuation, WEEKLY_ID},
};
use gridcraft_server::{
    providers::{OpenAiClient, OpenAiConfig},
    storage::{StorageError, Store, migrate},
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn model(State(fail_final): State<Arc<AtomicBool>>, Json(body): Json<Value>) -> Response {
    let messages = body["messages"].as_array().unwrap();
    let results = messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| serde_json::from_str::<Value>(m["content"].as_str().unwrap()).unwrap())
        .collect::<Vec<_>>();
    let unsolicited = messages
        .iter()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .is_some_and(|text| text.contains("Do not remember"));
    let tool = if unsolicited {
        if results.is_empty() { Some(("unsolicited", "remember_rule", json!({"instruction":"Remember to exclude test orders"}))) } else { None }
    } else {
        match results.len() {
            0 => Some(("compare", "compare_sales", json!({"dataset":"workspace_sales_records","periods":ComparisonPeriods::demo()}))),
            1 => Some(("context", "workbook_context", json!({}))),
            2 => Some(("inspect", "inspect_range", json!({"expected":results[1]["identity"],"range":"A1:E14"}))),
            3 => Some(("write", "write_range", json!({"expected":results[1]["identity"],"range":"A1:E14","values":results[0]["sheet_values"]}))),
            _ => None,
        }
    };
    if tool.is_none() && fail_final.swap(false, Ordering::SeqCst) {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    let (message, finish) = if let Some((id, name, args)) = tool {
        (
            json!({"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]}),
            "tool_calls",
        )
    } else {
        (json!({"role":"assistant","content":"Review the completed tool results and source evidence."}), "stop")
    };
    Json(json!({"choices":[{"message":message,"finish_reason":finish}]})).into_response()
}

async fn setup(fail: bool) -> Result<(PgPool, Store, OpenAiClient, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let url = std::env::var("GRIDCRAFT_TEST_DATABASE_URL")?;
    let pool = PgPoolOptions::new().max_connections(5).connect(&url).await.map_err(|_| StorageError::Database)?;
    migrate(&pool).await?;
    let store = Store::new(pool.clone(), Uuid::new_v4());
    store.seed_demo().await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, Router::new().route("/v1/chat/completions", post(model)).with_state(Arc::new(AtomicBool::new(fail)))).await;
    });
    let client = OpenAiClient::new(OpenAiConfig {
        api_key: "local-test-only".into(),
        base_url: format!("http://{address}/v1"),
        model: "scripted-test".into(),
    })?;
    Ok((pool, store, client, task))
}

async fn cleanup(pool: &PgPool, run: Uuid) -> TestResult {
    let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM review_agent_runs WHERE run_id=$1").bind(run).fetch_one(pool).await?;
    for table in ["review_agent_runs", "review_snapshots", "review_workbooks", "review_queries", "review_rules", "review_sales", "review_workspaces"]
    {
        // Table names are static test-owned constants; workspace remains parameterized.
        sqlx::query(&format!("DELETE FROM {table} WHERE workspace_id=$1")).bind(workspace).execute(pool).await?;
    }
    Ok(())
}

async fn until_write(
    store: &Store,
    client: &OpenAiClient,
    session: &mut Session,
    bridge: &mut BridgeState,
) -> Result<(Uuid, gridcraft_engine::workspace::RunView), Box<dyn std::error::Error>> {
    let run = Uuid::new_v4();
    let request = StartRun {
        run_id: run.to_string(),
        workbook_session_id: "test-live-editor".into(),
        prompt: "Compare sales".into(),
        context: json!(bridge.observe(session)?),
        attachments: vec![],
    };
    store.start_run(request.clone()).await?;
    assert_eq!(store.start_run(request.clone()).await?.run_id, run.to_string());
    let mut changed = request;
    changed.prompt = "different request".into();
    assert!(matches!(store.start_run(changed).await, Err(StorageError::Conflict)));
    let mut view = store.advance_run(run, client, None).await?;
    for expected_method in ["context", "inspect"] {
        let pending = view.pending.clone().ok_or("missing browser tool")?;
        assert_eq!(pending.method, expected_method);
        let result = bridge.handle(session, &pending.method, pending.params)?;
        view = store
            .advance_run(
                run,
                client,
                Some(ToolContinuation { workbook_session_id: "test-live-editor".into(), tool_call_id: pending.tool_call_id, result }),
            )
            .await?;
    }
    assert_eq!(view.pending.as_ref().ok_or("missing write")?.method, "set_values");
    assert_eq!(view.sources[0]["review"]["total"]["current_minor"], 65500);
    Ok((run, view))
}

#[tokio::test]
#[ignore = "requires explicit disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn durable_receipts_survive_provider_failure_and_concurrent_retries() -> TestResult {
    let (pool, store, client, task) = setup(true).await?;
    store.seed_files().await?;
    store.seed_files().await?;
    let files = store.list_workbooks().await?;
    assert_eq!(files.len(), 3);
    assert!(files.iter().all(|f| f.latest_version == 1));
    let original = store.snapshot_version(Uuid::parse_str(WEEKLY_ID)?, 1).await?;
    let mut session = Session::new();
    session.execute("file.open", json!({"name":"Weekly Sales.xlsx","base64":gridcraft_engine::io::base64_encode(&original.xlsx)}))?;
    let mut bridge = BridgeState::default();
    let (run, view) = until_write(&store, &client, &mut session, &mut bridge).await?;
    let pending = view.pending.ok_or("missing write")?;
    let result = bridge.handle(&mut session, &pending.method, pending.params.clone())?;
    assert_eq!(bridge.handle(&mut session, &pending.method, pending.params)?, result);
    assert_eq!(session.doc()?.undo.len(), 1);
    let receipt = ToolContinuation { workbook_session_id: "test-live-editor".into(), tool_call_id: pending.tool_call_id, result };
    let interrupted = store.advance_run(run, &client, Some(receipt.clone())).await?;
    assert_eq!(interrupted.status, "interrupted");
    assert_eq!(interrupted.completed_actions.len(), 1);
    // New repository/connection verifies recovery reads persisted JSON, not a local run map.
    let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM review_agent_runs WHERE run_id=$1").bind(run).fetch_one(&pool).await?;
    let restarted_pool = PgPoolOptions::new().max_connections(5).connect(&std::env::var("GRIDCRAFT_TEST_DATABASE_URL")?).await?;
    let restarted = Store::new(restarted_pool.clone(), workspace);
    assert_eq!(restarted.run_view(run).await?.completed_actions.len(), 1);
    let (one, two) =
        tokio::join!(restarted.advance_run(run, &client, Some(receipt.clone())), restarted.advance_run(run, &client, Some(receipt.clone())));
    assert_eq!(one?.status, "complete");
    assert_eq!(two?.completed_actions.len(), 1);
    let mut wrong = receipt.clone();
    wrong.workbook_session_id = "new-editor".into();
    assert!(restarted.advance_run(run, &client, Some(wrong)).await.is_err());
    let mut changed = receipt;
    changed.result = json!({"applied":false});
    assert!(matches!(restarted.advance_run(run, &client, Some(changed)).await, Err(StorageError::Conflict)));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM review_queries WHERE workspace_id=$1").bind(workspace).fetch_one(&pool).await?;
    assert_eq!(count, 1, "recovery does not repeat the business query");
    assert_eq!(session.doc()?.undo.len(), 1, "recovery does not execute the write again");
    cleanup(&pool, run).await?;
    restarted_pool.close().await;
    pool.close().await;
    task.abort();
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn stale_proposals_keep_manual_edits_and_do_not_count_failed_actions() -> TestResult {
    let (pool, store, client, task) = setup(false).await?;
    let mut session = Session::new();
    session.new_workbook();
    let mut bridge = BridgeState::default();
    let (run, view) = until_write(&store, &client, &mut session, &mut bridge).await?;
    let pending = view.pending.ok_or("missing write")?;
    session.execute("cell.set", json!({"cell":"A1","input":"manual correction"}))?;
    let error = bridge.handle(&mut session, &pending.method, pending.params).expect_err("stale proposal must fail");
    let result = store
        .advance_run(
            run,
            &client,
            Some(ToolContinuation {
                workbook_session_id: "test-live-editor".into(),
                tool_call_id: pending.tool_call_id,
                result: json!({"error":error.to_string()}),
            }),
        )
        .await?;
    assert_eq!(result.status, "complete");
    assert!(result.completed_actions.is_empty());
    assert_eq!(session.doc()?.wb.active().ok_or("sheet missing")?.value(gridcraft_engine::core::CellRef::default()).display(), "manual correction");
    assert_eq!(session.doc()?.undo.len(), 1);
    cleanup(&pool, run).await?;
    pool.close().await;
    task.abort();
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn model_cannot_save_a_rule_without_explicit_user_authorization() -> TestResult {
    let (pool, store, client, task) = setup(false).await?;
    let run = Uuid::new_v4();
    store
        .start_run(StartRun {
            run_id: run.to_string(),
            workbook_session_id: "editor".into(),
            prompt: "Do not remember any rules; just inspect my workbook".into(),
            context: json!({}),
            attachments: vec![],
        })
        .await?;
    let view = store.advance_run(run, &client, None).await?;
    assert_eq!(view.status, "complete");
    assert!(view.completed_actions.is_empty());
    assert!(store.list_rules().await?.is_empty());
    cleanup(&pool, run).await?;
    pool.close().await;
    task.abort();
    Ok(())
}
