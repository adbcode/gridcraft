//! Durable, bounded server-side agent runs. The browser executes workbook tools; the
//! server executes database tools and records every tool result as a receipt.

use gridcraft_engine::{
    agent_protocol::{AttachmentRef, PendingTool, RunContinue, RunStart, RunView, SourceRef, WritePlan},
    review_workflow::{ComparisonPeriods, SalesReview},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

use crate::{
    providers::{ChatMessage, OpenAiClient, ProviderError, ToolCall, ToolDefinition},
    storage::{StorageError, Store},
};

const MAX_MODEL_CALLS: i32 = 8;
const STALE_SECONDS: i32 = 90;
const RULE_ID: &str = "exclude-tests-v1";
const SYSTEM_PROMPT: &str = include_str!("agent_system_prompt.txt");
const BROWSER_TOOLS: &[&str] = &["get_context", "inspect_range", "write_range", "write_sales_review", "undo_last"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RunError {
    #[error("run was not found")]
    NotFound,
    #[error("run request conflicts with an existing operation")]
    Conflict,
    #[error("run input is invalid")]
    Invalid,
    #[error("model provider is not configured")]
    Unconfigured,
    #[error("storage request failed")]
    Storage,
    #[error("stored run is invalid")]
    Corrupt,
}

impl From<StorageError> for RunError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::NotFound => Self::NotFound,
            StorageError::Conflict => Self::Conflict,
            StorageError::InvalidInput(_) | StorageError::Review(_) | StorageError::LimitExceeded => Self::Invalid,
            StorageError::CorruptData => Self::Corrupt,
            StorageError::Database | StorageError::Migration => Self::Storage,
        }
    }
}

impl From<sqlx::Error> for RunError {
    fn from(_: sqlx::Error) -> Self {
        Self::Storage
    }
}

type Result<T> = std::result::Result<T, RunError>;

#[derive(Clone)]
pub struct RunService {
    store: Store,
    openai: Option<OpenAiClient>,
}

/// One tool's outcome inside the server loop.
enum Step {
    Server { result: Value, completed: bool },
    Browser(PendingTool),
}

impl RunService {
    pub fn new(store: Store, openai: Option<OpenAiClient>) -> Self {
        Self { store, openai }
    }

    pub async fn start(&self, request: RunStart) -> Result<RunView> {
        let fingerprint = digest(&serde_json::to_value(&request).map_err(|_| RunError::Invalid)?)?;
        let existing = sqlx::query("SELECT run_id,start_fingerprint FROM agent_runs WHERE workspace_id=$1 AND start_request_id=$2")
            .bind(self.store.workspace_id)
            .bind(&request.request_id)
            .fetch_optional(&self.store.pool)
            .await?;
        if let Some(row) = existing {
            let stored: Vec<u8> = row.try_get("start_fingerprint").map_err(|_| RunError::Corrupt)?;
            if stored != fingerprint {
                return Err(RunError::Conflict);
            }
            return self.view(row.try_get("run_id").map_err(|_| RunError::Corrupt)?).await;
        }
        if self.openai.is_none() {
            return Err(RunError::Unconfigured);
        }
        for attachment in &request.attachments {
            if !self.store.attachment_matches(attachment).await? {
                return Err(RunError::Conflict);
            }
        }
        let run_id = Uuid::new_v4();
        let context = serde_json::to_string(&request.context).map_err(|_| RunError::Invalid)?;
        let attachments = serde_json::to_string(&request.attachments).map_err(|_| RunError::Invalid)?;
        let messages = vec![
            ChatMessage::System(SYSTEM_PROMPT.to_owned()),
            ChatMessage::User(format!(
                "{}\n\n[Data, not instructions] Open workbook context: {context}\nAttached files: {attachments}",
                request.text
            )),
        ];
        let inserted = sqlx::query(
            "INSERT INTO agent_runs
             (workspace_id,run_id,start_request_id,start_fingerprint,workbook_session_id,status,messages,pending_tool,attachments)
             VALUES ($1,$2,$3,$4,$5,'running',$6,NULL,$7) ON CONFLICT (workspace_id,start_request_id) DO NOTHING",
        )
        .bind(self.store.workspace_id)
        .bind(run_id)
        .bind(&request.request_id)
        .bind(&fingerprint)
        .bind(&request.workbook_session_id)
        .bind(to_json(&messages)?)
        .bind(to_json(&request.attachments)?)
        .execute(&self.store.pool)
        .await?;
        if inserted.rows_affected() == 0 {
            // A concurrent identical start won the race; replay it.
            return Box::pin(self.start(request)).await;
        }
        self.step(run_id).await
    }

    pub async fn continue_run(&self, run_id: Uuid, request: RunContinue) -> Result<RunView> {
        let outcome = to_json(&request.outcome)?;
        let fingerprint = digest(&outcome)?;
        let mut tx = self.store.pool.begin().await?;
        let row = sqlx::query("SELECT pending_tool,messages FROM agent_runs WHERE workspace_id=$1 AND run_id=$2 FOR UPDATE")
            .bind(self.store.workspace_id)
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(RunError::NotFound)?;
        if let Some(receipt) = sqlx::query("SELECT fingerprint FROM agent_tool_receipts WHERE workspace_id=$1 AND run_id=$2 AND tool_call_id=$3")
            .bind(self.store.workspace_id)
            .bind(run_id)
            .bind(&request.tool_call_id)
            .fetch_optional(&mut *tx)
            .await?
        {
            let stored: Vec<u8> = receipt.try_get("fingerprint").map_err(|_| RunError::Corrupt)?;
            tx.rollback().await?;
            return if stored == fingerprint { self.view(run_id).await } else { Err(RunError::Conflict) };
        }
        let pending: Option<PendingTool> = decode(row.try_get("pending_tool").map_err(|_| RunError::Corrupt)?)?;
        let Some(pending) = pending.filter(|pending| pending.tool_call_id == request.tool_call_id) else {
            return Err(RunError::Conflict);
        };
        let mut messages: Vec<ChatMessage> = decode(Some(row.try_get("messages").map_err(|_| RunError::Corrupt)?))?.ok_or(RunError::Corrupt)?;
        messages.push(ChatMessage::ToolResult { tool_call_id: pending.tool_call_id.clone(), content: outcome.to_string() });
        let completed = request.outcome.ok && (pending.write.is_some() || pending.name == "undo_last");
        insert_receipt(&mut tx, &self.store, run_id, &pending.tool_call_id, &pending.name, &fingerprint, &outcome, completed).await?;
        sqlx::query(
            "UPDATE agent_runs SET status='running',messages=$3,pending_tool=NULL,action_count=action_count+$4,updated_at=now()
             WHERE workspace_id=$1 AND run_id=$2",
        )
        .bind(self.store.workspace_id)
        .bind(run_id)
        .bind(to_json(&messages)?)
        .bind(i32::from(completed))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.step(run_id).await
    }

    pub async fn get(&self, run_id: Uuid) -> Result<RunView> {
        self.view(run_id).await
    }

    pub async fn resume(&self, run_id: Uuid) -> Result<RunView> {
        let updated = sqlx::query(
            "UPDATE agent_runs SET status='running',error=NULL,updated_at=now()
             WHERE workspace_id=$1 AND run_id=$2 AND pending_tool IS NULL AND (status='interrupted'
               OR (status='running' AND updated_at < now() - make_interval(secs => $3)))",
        )
        .bind(self.store.workspace_id)
        .bind(run_id)
        .bind(f64::from(STALE_SECONDS))
        .execute(&self.store.pool)
        .await?;
        if updated.rows_affected() == 0 {
            self.view(run_id).await?;
            return Err(RunError::Conflict);
        }
        self.step(run_id).await
    }

    async fn step(&self, run_id: Uuid) -> Result<RunView> {
        let Some(client) = &self.openai else {
            return Err(RunError::Unconfigured);
        };
        loop {
            let row = self.row(run_id).await?;
            let calls: i32 = row.try_get("model_calls").map_err(|_| RunError::Corrupt)?;
            if calls >= MAX_MODEL_CALLS {
                self.finish(run_id, "failed", None, Some("model_call_limit")).await?;
                return self.view(run_id).await;
            }
            let mut messages: Vec<ChatMessage> = decode(Some(row.try_get("messages").map_err(|_| RunError::Corrupt)?))?.ok_or(RunError::Corrupt)?;
            let attachments: Vec<AttachmentRef> =
                decode(Some(row.try_get("attachments").map_err(|_| RunError::Corrupt)?))?.ok_or(RunError::Corrupt)?;
            let completion = match client.complete(&messages, &tool_definitions()).await {
                Ok(completion) => completion,
                Err(error) => {
                    self.finish(run_id, "interrupted", None, Some(provider_code(&error))).await?;
                    return self.view(run_id).await;
                }
            };
            let tool_calls = completion.assistant.tool_calls.clone();
            let text = completion.assistant.text.clone();
            messages.push(ChatMessage::Assistant(completion.assistant));
            let mut tx = self.store.pool.begin().await?;
            let mut pending = None;
            let mut actions = 0;
            for call in &tool_calls {
                if pending.is_some() {
                    let deferred = json!({"ok":false,"error":"deferred: request this tool again after the previous result"});
                    messages.push(ChatMessage::ToolResult { tool_call_id: call.id.clone(), content: deferred.to_string() });
                    continue;
                }
                match self.execute(call, &attachments).await {
                    Step::Server { result, completed } => {
                        insert_receipt(&mut tx, &self.store, run_id, &call.id, &call.name, &digest(&result)?, &result, completed).await?;
                        actions += i32::from(completed);
                        messages.push(ChatMessage::ToolResult { tool_call_id: call.id.clone(), content: result.to_string() });
                    }
                    Step::Browser(tool) => pending = Some(tool),
                }
            }
            let status = match (&pending, tool_calls.is_empty()) {
                (Some(_), _) => "awaiting_tool",
                (None, true) => "completed",
                (None, false) => "running",
            };
            sqlx::query(
                "UPDATE agent_runs SET status=$3,messages=$4,pending_tool=$5,model_calls=model_calls+1,
                 action_count=action_count+$6,final_text=$7,updated_at=now() WHERE workspace_id=$1 AND run_id=$2",
            )
            .bind(self.store.workspace_id)
            .bind(run_id)
            .bind(status)
            .bind(to_json(&messages)?)
            .bind(pending.as_ref().map(to_json).transpose()?)
            .bind(actions)
            .bind(if status == "completed" { text.map(|text| truncate(&text, 16_000)) } else { None })
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            if status != "running" {
                return self.view(run_id).await;
            }
        }
    }

    /// Server tools never fail the run: errors are returned to the model as tool data.
    async fn execute(&self, call: &ToolCall, attachments: &[AttachmentRef]) -> Step {
        if BROWSER_TOOLS.contains(&call.name.as_str()) {
            return match self.browser_tool(call).await {
                Ok(tool) => Step::Browser(tool),
                Err(code) => Step::Server { result: json!({"ok":false,"error":code}), completed: false },
            };
        }
        let arg = |name: &str| call.arguments.get(name).and_then(Value::as_str).unwrap_or_default().to_owned();
        let result: std::result::Result<(Value, bool), &str> = match call.name.as_str() {
            "compare_sales" => match self.store.compare(ComparisonPeriods::demo()).await {
                Ok(review) => Ok((review_summary(&review), false)),
                Err(_) => Err("sales_query_failed"),
            },
            "remember_rule" => match self.store.save_rule(RULE_ID, &arg("instruction")).await {
                Ok(rule) => Ok((json!({"saved":true,"rule":rule}), true)),
                Err(StorageError::Review(_) | StorageError::InvalidInput(_)) => Err("unsupported_rule_wording"),
                Err(_) => Err("rule_not_saved"),
            },
            "list_rules" => self.store.list_rules().await.map(|rules| (json!({"rules":rules}), false)).map_err(|_| "rules_unavailable"),
            "get_query_evidence" => match Uuid::parse_str(&arg("query_id")) {
                Ok(id) => self.store.query_evidence(id).await.map(|evidence| (json!({"evidence":evidence}), false)).map_err(|_| "evidence_not_found"),
                Err(_) => Err("invalid_query_id"),
            },
            "search_files" => self
                .store
                .list_files(&arg("q"))
                .await
                .map(|files| (json!({"files":files,"label":"Mock cloud library"}), false))
                .map_err(|_| "search_failed"),
            "read_attached_file" => {
                let version = call.arguments.get("version").and_then(Value::as_i64).unwrap_or_default();
                let id = arg("workbook_id");
                match (attachments.iter().find(|item| item.workbook_id == id && item.version == version), Uuid::parse_str(&id)) {
                    (Some(_), Ok(uuid)) => self
                        .store
                        .preview_snapshot(uuid, version)
                        .await
                        .map(|preview| (json!({"file":preview}), false))
                        .map_err(|_| "file_unreadable"),
                    _ => Err("not_attached"),
                }
            }
            _ => Err("unknown_tool"),
        };
        match result {
            Ok((value, completed)) => Step::Server { result: json!({"ok":true,"result":value}), completed },
            Err(code) => Step::Server { result: json!({"ok":false,"error":code}), completed: false },
        }
    }

    async fn browser_tool(&self, call: &ToolCall) -> std::result::Result<PendingTool, &'static str> {
        let write = match call.name.as_str() {
            "write_sales_review" => {
                let id = call.arguments.get("query_id").and_then(Value::as_str).and_then(|id| Uuid::parse_str(id).ok()).ok_or("invalid_query_id")?;
                let evidence = self.store.query_evidence(id).await.map_err(|_| "evidence_not_found")?;
                let periods = evidence
                    .provenance
                    .get("periods")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_else(ComparisonPeriods::demo);
                let review = SalesReview::rebuild_from_evidence(&evidence.source_records, &evidence.rule_context, &id.to_string(), periods)
                    .map_err(|_| "evidence_invalid")?;
                Some(WritePlan { sheet: "Review".into(), range: "A1".into(), values: review.sheet_values_eur() })
            }
            "write_range" => {
                let plan = json!({
                    "sheet": call.arguments.get("sheet").and_then(Value::as_str).unwrap_or("Review"),
                    "range": call.arguments.get("range").cloned().unwrap_or(Value::Null),
                    "values": call.arguments.get("values").cloned().unwrap_or(Value::Null),
                });
                Some(serde_json::from_value(plan).map_err(|_| "invalid_write_plan")?)
            }
            _ => None,
        };
        let arguments = serde_json::from_value(json!(call.arguments)).map_err(|_| "invalid_arguments")?;
        Ok(PendingTool { tool_call_id: call.id.clone(), name: call.name.clone(), arguments, write })
    }

    async fn finish(&self, run_id: Uuid, status: &str, text: Option<String>, error: Option<&str>) -> Result<()> {
        sqlx::query(
            "UPDATE agent_runs SET status=$3,final_text=COALESCE($4,final_text),error=$5,updated_at=now() WHERE workspace_id=$1 AND run_id=$2",
        )
        .bind(self.store.workspace_id)
        .bind(run_id)
        .bind(status)
        .bind(text)
        .bind(error)
        .execute(&self.store.pool)
        .await?;
        Ok(())
    }

    async fn row(&self, run_id: Uuid) -> Result<PgRow> {
        sqlx::query(
            "SELECT *, (status='running' AND updated_at < now() - make_interval(secs => $3)) AS stale
             FROM agent_runs WHERE workspace_id=$1 AND run_id=$2",
        )
        .bind(self.store.workspace_id)
        .bind(run_id)
        .bind(f64::from(STALE_SECONDS))
        .fetch_optional(&self.store.pool)
        .await?
        .ok_or(RunError::NotFound)
    }

    async fn view(&self, run_id: Uuid) -> Result<RunView> {
        let row = self.row(run_id).await?;
        let stale: bool = row.try_get("stale").map_err(|_| RunError::Corrupt)?;
        let status: String = if stale { "interrupted".into() } else { row.try_get("status").map_err(|_| RunError::Corrupt)? };
        let receipts =
            sqlx::query("SELECT tool_name,result FROM agent_tool_receipts WHERE workspace_id=$1 AND run_id=$2 ORDER BY created_at,tool_call_id")
                .bind(self.store.workspace_id)
                .bind(run_id)
                .fetch_all(&self.store.pool)
                .await?;
        let attachments: Vec<AttachmentRef> = decode(Some(row.try_get("attachments").map_err(|_| RunError::Corrupt)?))?.unwrap_or_default();
        let mut sources: Vec<SourceRef> = Vec::new();
        for receipt in &receipts {
            let tool: String = receipt.try_get("tool_name").map_err(|_| RunError::Corrupt)?;
            let result: Value = receipt.try_get("result").map_err(|_| RunError::Corrupt)?;
            let source = match tool.as_str() {
                "compare_sales" => result.pointer("/result/query_id").and_then(Value::as_str).map(|id| SourceRef {
                    kind: "sales_query".into(),
                    label: format!(
                        "Synthetic demo sales (rules applied: {})",
                        result.pointer("/result/rules_applied").and_then(Value::as_array).map(|rules| rules.len()).unwrap_or_default()
                    ),
                    id: id.into(),
                    version: None,
                }),
                "remember_rule" if result.get("ok") == Some(&Value::Bool(true)) => {
                    Some(SourceRef { kind: "rule".into(), label: "Exclude test orders".into(), id: RULE_ID.into(), version: None })
                }
                _ => None,
            };
            if let Some(source) = source.filter(|source| !sources.contains(source)) {
                sources.push(source);
            }
        }
        for attachment in attachments {
            sources.push(SourceRef {
                kind: "workbook".into(),
                label: "Attached file".into(),
                id: attachment.workbook_id,
                version: Some(attachment.version),
            });
        }
        sources.truncate(32);
        let final_text: Option<String> = row.try_get("final_text").map_err(|_| RunError::Corrupt)?;
        let error: Option<String> = row.try_get("error").map_err(|_| RunError::Corrupt)?;
        let spoken_summary = (status == "completed").then(|| final_text.as_deref().map(|text| truncate(text, 600))).flatten();
        Ok(RunView {
            run_id: run_id.to_string(),
            status,
            pending_tool: decode(row.try_get("pending_tool").map_err(|_| RunError::Corrupt)?)?,
            final_text: final_text.or(error.map(|code| format!("Run stopped: {code}"))),
            action_count: row.try_get("action_count").map_err(|_| RunError::Corrupt)?,
            sources,
            spoken_summary,
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    store: &Store,
    run_id: Uuid,
    tool_call_id: &str,
    tool_name: &str,
    fingerprint: &[u8],
    result: &Value,
    completed: bool,
) -> Result<()> {
    if tool_call_id.is_empty() || tool_call_id.len() > 128 || tool_name.is_empty() || tool_name.len() > 128 {
        return Err(RunError::Invalid);
    }
    sqlx::query(
        "INSERT INTO agent_tool_receipts (workspace_id,run_id,tool_call_id,tool_name,fingerprint,result,completed)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(store.workspace_id)
    .bind(run_id)
    .bind(tool_call_id)
    .bind(tool_name)
    .bind(fingerprint)
    .bind(result)
    .bind(completed)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn review_summary(review: &SalesReview) -> Value {
    let euros = |minor: i64| format!("{:.2}", minor as f64 / 100.0);
    let provenance = review.provenance();
    json!({
        "query_id": provenance.query_id,
        "dataset": "Synthetic demo sales (mock workspace)",
        "periods": "previous 2026-10-05..2026-10-12 UTC, current 2026-10-12..2026-10-19 UTC (end excluded)",
        "metric": "paid-order gross minus refunds, EUR",
        "rules_applied": provenance.applied_rule_ids,
        "record_counts": {"source": provenance.source_record_count, "previous": provenance.previous_record_count, "current": provenance.current_record_count},
        "categories": review.categories().iter().map(|c| json!({"category": c.category, "previous_eur": euros(c.previous_minor), "current_eur": euros(c.current_minor)})).collect::<Vec<_>>(),
        "total": {"previous_eur": euros(review.total().previous_minor), "current_eur": euros(review.total().current_minor)},
    })
}

fn provider_code(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::InvalidConfiguration => "provider_configuration_invalid",
        _ => "provider_unavailable",
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.char_indices().take_while(|(index, ch)| index + ch.len_utf8() <= max).map(|(_, ch)| ch).collect()
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|_| RunError::Invalid)
}

fn digest(value: &Value) -> Result<Vec<u8>> {
    Ok(Sha256::digest(serde_json::to_vec(value).map_err(|_| RunError::Invalid)?).to_vec())
}

fn decode<T: serde::de::DeserializeOwned>(value: Option<Value>) -> Result<Option<T>> {
    value.map(|value| serde_json::from_value(value).map_err(|_| RunError::Corrupt)).transpose()
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    }
}

pub fn tool_definitions() -> Vec<ToolDefinition> {
    let text = json!({"type":"string"});
    vec![
        tool("get_context", "Identity, selection and dirty state of the open workbook.", json!({}), &[]),
        tool("inspect_range", "Read up to 1000 cells of the open workbook's active sheet, e.g. A1:E14.", json!({"range":text}), &["range"]),
        tool(
            "write_range",
            "Propose writing a rectangular block (max 1000 cells) to the open workbook. The user must press Apply.",
            json!({"sheet":text,"range":text,"values":{"type":"array","items":{"type":"array","items":{}}},"summary":text}),
            &["range", "values", "summary"],
        ),
        tool(
            "write_sales_review",
            "Propose writing the stored sales comparison with this query_id to the Review sheet at A1. The user must press Apply.",
            json!({"query_id":text}),
            &["query_id"],
        ),
        tool("undo_last", "Undo the last workbook operation with native undo.", json!({}), &[]),
        tool(
            "compare_sales",
            "Compare net revenue between the previous and current complete UTC weeks using stored sales and remembered rules.",
            json!({}),
            &[],
        ),
        tool(
            "remember_rule",
            "Persist a rule the user explicitly asked to remember. Supported: excluding test orders.",
            json!({"instruction":text}),
            &["instruction"],
        ),
        tool("list_rules", "List remembered rules.", json!({}), &[]),
        tool("get_query_evidence", "Read the immutable evidence for a sales query.", json!({"query_id":text}), &["query_id"]),
        tool("search_files", "Search the mock cloud workbook library by title.", json!({"q":text}), &["q"]),
        tool(
            "read_attached_file",
            "Read the first rows of an attached workbook version.",
            json!({"workbook_id":text,"version":{"type":"integer"}}),
            &["workbook_id", "version"],
        ),
    ]
}
