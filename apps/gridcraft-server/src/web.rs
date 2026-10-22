//! Same-origin HTTP API. The workspace is selected by the server, never request input.
use std::{path::PathBuf, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use gridcraft_engine::review_workflow::{ComparisonPeriods, Sale};
use serde::Deserialize;
use serde_json::json;
use tower_http::{services::ServeDir, timeout::TimeoutLayer};
use uuid::Uuid;

use crate::storage::{SnapshotMetadata, SnapshotUpload, StorageError, Store};

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub store: Option<Store>,
    pub static_dir: PathBuf,
    pub openai_configured: bool,
    pub speech_configured: bool,
    pub tts_configured: bool,
    pub openai: Option<crate::providers::OpenAiClient>,
    pub elevenlabs: Option<crate::providers::ElevenLabsClient>,
}

pub fn router(state: AppState) -> Router {
    let assets = ServeDir::new(&state.static_dir).append_index_html_on_directories(true);
    let snapshots = Router::new().route("/api/workbooks/{id}/snapshots", post(upload_snapshot)).layer(DefaultBodyLimit::max(MAX_FILE_BYTES + 65_536));
    let speech = Router::new().route("/api/speech/transcribe", post(transcribe)).layer(DefaultBodyLimit::max(crate::providers::MAX_AUDIO_BYTES));
    Router::new()
        .route("/api/runs", post(start_run))
        .route("/api/runs/{id}", get(run_status))
        .route("/api/runs/{id}/continue", post(continue_run))
        .route("/api/runs/{id}/resume", post(resume_run))
        .route("/api/runs/{id}/speech", post(speak_run))
        .route("/api/library", get(library))
        .route("/api/workbooks/{id}/snapshots/{version}", get(download_version))
        .route("/api/health/live", get(|| async { Json(json!({"status":"alive"})) }))
        .route("/api/health/ready", get(ready))
        .route("/api/status", get(status))
        .route("/api/rules", get(list_rules).post(save_rule))
        .route("/api/sales/compare", post(compare))
        .route("/api/sales/import", post(import_sales))
        .route("/api/queries/{id}", get(query_evidence))
        .route("/api/workbooks", get(list_workbooks))
        .route("/api/workbooks/{id}", put(create_workbook))
        .route("/api/workbooks/{id}/snapshots/latest", get(download_snapshot))
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES))
        .merge(snapshots)
        .merge(speech)
        .fallback_service(assets)
        .layer(middleware::from_fn(same_origin))
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(120)))
        .with_state(state)
}

async fn same_origin(request: Request, next: Next) -> Response {
    // This is a single mocked workspace, not user authentication. Reject browser requests
    // from another origin and hidden files; do not serve the checkout as an asset root.
    let path = request.uri().path().to_ascii_lowercase();
    if path.contains(['\\'])
        || path.contains("%2f")
        || path.contains("%5c")
        || path.split('/').any(|segment| segment.starts_with('.') || segment.contains("%2e"))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Schemeful browser fetch metadata also rejects http -> https on the same host.
    // Apply this only to the API so links from the separate landing page still work.
    if path.starts_with("/api/") && request.headers().get("sec-fetch-site").is_some_and(|value| !matches!(value.to_str(), Ok("same-origin" | "none")))
    {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"cross_origin_request"}))).into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let host = request.headers().get(header::HOST).and_then(|host| host.to_str().ok());
        let origin = origin.to_str().ok().and_then(|origin| origin.parse::<axum::http::Uri>().ok());
        if !origin.as_ref().is_some_and(|origin| {
            matches!(origin.scheme_str(), Some("http" | "https")) && origin.authority().map(|authority| authority.as_str()) == host
        }) {
            return (StatusCode::FORBIDDEN, Json(json!({"error":"cross_origin_request"}))).into_response();
        }
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(header::X_CONTENT_TYPE_OPTIONS, axum::http::HeaderValue::from_static("nosniff"));
    response
}

async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "workspace_mode": "single_mock_user", "authentication": false,
        "ui_workflow": "connected_when_configured", "durable_agent_loop": state.store.is_some() && state.openai.is_some(),
        "database_configured": state.store.is_some(),
        "openai_configured": state.openai_configured,
        "elevenlabs_stt_configured": state.speech_configured,
        "elevenlabs_tts_configured": state.tts_configured,
        "provider_connectivity": "not_probed"
    }))
}

async fn ready(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    store(&state)?.check_ready().await?;
    if !state.static_dir.join("index.html").is_file() {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "web_assets_missing"));
    }
    Ok(Json(json!({"status":"ready", "database":"reachable", "schema":"available", "web_assets":"available", "providers":"not_probed"})))
}

fn store(state: &AppState) -> Result<&Store, ApiError> {
    state.store.as_ref().ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "database_not_configured"))
}

async fn list_rules(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(store(&state)?.list_rules().await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleRequest {
    id: String,
    instruction: String,
}

async fn save_rule(State(state): State<AppState>, Json(request): Json<RuleRequest>) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(store(&state)?.save_rule(&request.id, &request.instruction).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompareRequest {
    periods: ComparisonPeriods,
}

async fn compare(State(state): State<AppState>, Json(request): Json<CompareRequest>) -> Result<impl IntoResponse, ApiError> {
    let review = store(&state)?.compare(request.periods).await?;
    let evidence_url = format!("/api/queries/{}", review.provenance().query_id);
    Ok(Json(json!({"review": review, "sheet_values": review.sheet_values(), "evidence_url":evidence_url})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRequest {
    records: Vec<Sale>,
}

async fn import_sales(State(state): State<AppState>, Json(request): Json<ImportRequest>) -> Result<impl IntoResponse, ApiError> {
    let result = store(&state)?.upsert_sales(&request.records).await?;
    Ok(Json(json!({"result": result})))
}

async fn query_evidence(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(store(&state)?.query_evidence(id).await?))
}

async fn list_workbooks(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(store(&state)?.list_workbooks().await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkbookRequest {
    title: String,
}

async fn create_workbook(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<WorkbookRequest>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(store(&state)?.create_workbook(id, &request.title).await?))
}

async fn upload_snapshot(State(state): State<AppState>, Path(id): Path<Uuid>, mut form: Multipart) -> Result<impl IntoResponse, ApiError> {
    let repository = store(&state)?;
    let mut metadata = None;
    let mut xlsx = None;
    while let Some(mut field) = form.next_field().await.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_snapshot_multipart"))? {
        let is_metadata = match field.name() {
            Some("metadata") if metadata.is_none() => true,
            Some("file") if xlsx.is_none() => false,
            _ => return Err(ApiError(StatusCode::BAD_REQUEST, "unexpected_or_duplicate_snapshot_field")),
        };
        let limit = if is_metadata { 16_384 } else { MAX_FILE_BYTES };
        let mut bytes = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_snapshot_multipart"))? {
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "snapshot_too_large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if is_metadata {
            metadata =
                Some(serde_json::from_slice::<SnapshotMetadata>(&bytes).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_snapshot_metadata"))?);
        } else {
            xlsx = Some(bytes);
        }
    }
    let metadata = metadata.ok_or(ApiError(StatusCode::BAD_REQUEST, "snapshot_metadata_required"))?;
    let xlsx = xlsx.ok_or(ApiError(StatusCode::BAD_REQUEST, "snapshot_file_required"))?;
    // Bytes remain opaque: only the live editor's successful open can acknowledge a load.
    Ok(Json(repository.save_snapshot(SnapshotUpload { workbook_id: id, metadata, xlsx }).await?))
}

async fn download_snapshot(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<Response, ApiError> {
    let snapshot = store(&state)?.latest_snapshot(id).await?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, axum::http::HeaderValue::from_static("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
    headers.insert(header::CONTENT_DISPOSITION, axum::http::HeaderValue::from_static("attachment; filename=workbook.xlsx"));
    headers.insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    headers.insert(
        "x-gridcraft-storage-version",
        snapshot.receipt.storage_version.to_string().parse().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "invalid_snapshot_receipt"))?,
    );
    headers.insert(
        "x-gridcraft-sha256",
        snapshot.receipt.sha256.parse().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "invalid_snapshot_receipt"))?,
    );
    Ok((headers, Body::from(snapshot.xlsx)).into_response())
}

pub(crate) struct ApiError(StatusCode, &'static str);

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::NotFound => Self(StatusCode::NOT_FOUND, "not_found"),
            StorageError::Conflict => Self(StatusCode::CONFLICT, "conflicting_request"),
            StorageError::InvalidInput(_) | StorageError::Review(_) => Self(StatusCode::UNPROCESSABLE_ENTITY, "invalid_or_unsupported_input"),
            StorageError::LimitExceeded => Self(StatusCode::PAYLOAD_TOO_LARGE, "storage_limit_exceeded"),
            StorageError::Database | StorageError::Migration => Self(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable"),
            StorageError::CorruptData => Self(StatusCode::INTERNAL_SERVER_ERROR, "stored_data_invalid"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn app(path: PathBuf) -> Router {
        router(AppState {
            store: None,
            static_dir: path,
            openai_configured: false,
            speech_configured: false,
            tts_configured: false,
            openai: None,
            elevenlabs: None,
        })
    }

    #[tokio::test]
    async fn offline_status_does_not_claim_storage_or_providers() {
        let dir = tempfile::tempdir().unwrap();
        let router = app(dir.path().into());
        for (path, expected) in [
            ("/api/health/live", StatusCode::OK),
            ("/api/health/ready", StatusCode::SERVICE_UNAVAILABLE),
            ("/api/rules", StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let response = router.clone().oneshot(Request::builder().uri(path).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), expected);
        }
        let response = router.oneshot(Request::builder().uri("/api/status").body(Body::empty()).unwrap()).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status["provider_connectivity"], "not_probed");
        assert_eq!(status["durable_agent_loop"], false);
    }

    #[tokio::test]
    async fn static_assets_work_but_cross_origin_and_dotfiles_do_not() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "test-app").unwrap();
        std::fs::write(dir.path().join(".env"), "sensitive").unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested/.env"), "sensitive").unwrap();
        let router = app(dir.path().into());
        let response = router.clone().oneshot(Request::builder().uri("/").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        for path in ["/.env", "/%2Eenv", "/%2e%2e/.env", "/nested%2f.env", "/nested%5c.env"] {
            let response = router.clone().oneshot(Request::builder().uri(path).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/rules")
                    .header("host", "localhost:8080")
                    .header("origin", "https://unrelated.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn reject_owner_overrides_and_oversized_json() {
        let dir = tempfile::tempdir().unwrap();
        let router = app(dir.path().into());
        let request = Request::builder()
            .method("POST")
            .uri("/api/rules")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"id":"rule","instruction":"exclude test orders","workspace_id":"other"}"#))
            .unwrap();
        assert_eq!(router.clone().oneshot(request).await.unwrap().status(), StatusCode::UNPROCESSABLE_ENTITY);
        let request = Request::builder()
            .method("POST")
            .uri("/api/rules")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(vec![b' '; MAX_JSON_BYTES + 1]))
            .unwrap();
        assert_eq!(router.oneshot(request).await.unwrap().status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn reject_scheme_crossing_api_fetch_but_allow_landing_page_navigation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "test-app").unwrap();
        let router = app(dir.path().into());
        let request = Request::builder()
            .uri("/api/status")
            .header("host", "app.example")
            .header("origin", "http://app.example")
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap();
        assert_eq!(router.clone().oneshot(request).await.unwrap().status(), StatusCode::FORBIDDEN);
        let navigation = Request::builder().uri("/").header("sec-fetch-site", "cross-site").body(Body::empty()).unwrap();
        assert_eq!(router.oneshot(navigation).await.unwrap().status(), StatusCode::OK);
    }
}

async fn library(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let repository = store(&state)?;
    let mut files = Vec::new();
    for file in repository.list_workbooks().await?.into_iter().take(50) {
        if file.latest_version == 0 {
            continue;
        }
        let snapshot = repository.snapshot_version(file.workbook_id, file.latest_version).await?;
        let (wb, _) = gridcraft_engine::io::open_bytes(&file.title, &snapshot.xlsx)
            .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "stored_workbook_invalid"))?;
        let preview = wb
            .active()
            .map(|sheet| {
                (0..6)
                    .map(|r| {
                        (0..5)
                            .map(|c| json!(sheet.value(gridcraft_engine::core::CellRef::new(r, c)).display().chars().take(160).collect::<String>()))
                            .collect()
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.push(gridcraft_engine::workspace::LibraryFile {
            workbook_id: file.workbook_id.to_string(),
            title: file.title,
            latest_version: file.latest_version,
            source: "mock workspace storage".into(),
            preview,
        });
    }
    Ok(Json(files))
}

async fn download_version(State(state): State<AppState>, Path((id, version)): Path<(Uuid, i64)>) -> Result<Response, ApiError> {
    let snapshot = store(&state)?.snapshot_version(id, version).await?;
    let mut response = (
        [(header::CONTENT_TYPE, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"), (header::CACHE_CONTROL, "no-store")],
        snapshot.xlsx,
    )
        .into_response();
    response.headers_mut().insert(
        "x-gridcraft-storage-version",
        version.to_string().parse().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "invalid_version"))?,
    );
    response
        .headers_mut()
        .insert("x-gridcraft-sha256", snapshot.receipt.sha256.parse().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "invalid_digest"))?);
    Ok(response)
}

async fn start_run(State(state): State<AppState>, Json(request): Json<gridcraft_engine::workspace::StartRun>) -> Result<impl IntoResponse, ApiError> {
    let client = state.openai.as_ref().ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "openai_not_configured"))?;
    let view = store(&state)?.start_run(request).await?;
    let id = Uuid::parse_str(&view.run_id).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_run_id"))?;
    Ok(Json(store(&state)?.advance_run(id, client, None).await?))
}
async fn run_status(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(store(&state)?.run_view(id).await?))
}
async fn continue_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<gridcraft_engine::workspace::ToolContinuation>,
) -> Result<impl IntoResponse, ApiError> {
    let client = state.openai.as_ref().ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "openai_not_configured"))?;
    Ok(Json(store(&state)?.advance_run(id, client, Some(request)).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeRequest {
    workbook_session_id: String,
}
async fn resume_run(State(state): State<AppState>, Path(id): Path<Uuid>, Json(request): Json<ResumeRequest>) -> Result<impl IntoResponse, ApiError> {
    let client = state.openai.as_ref().ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "openai_not_configured"))?;
    let view = store(&state)?.run_view(id).await?;
    if view.workbook_session_id != request.workbook_session_id {
        return Err(ApiError(StatusCode::CONFLICT, "editor_session_changed_start_a_new_run"));
    }
    Ok(Json(store(&state)?.advance_run(id, client, None).await?))
}
async fn transcribe(State(state): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> Result<impl IntoResponse, ApiError> {
    let client = state.elevenlabs.as_ref().ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "speech_not_configured"))?;
    let mime =
        headers.get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).ok_or(ApiError(StatusCode::BAD_REQUEST, "audio_content_type_required"))?;
    Ok(Json(client.transcribe(body.to_vec(), mime).await.map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "transcription_failed"))?))
}
async fn speak_run(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<Response, ApiError> {
    let client = state.elevenlabs.as_ref().ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "speech_not_configured"))?;
    let view = store(&state)?.run_view(id).await?;
    // Speak a deterministic verified outcome, not unverified model prose or supplied text.
    let verified = view
        .completed_actions
        .iter()
        .filter(|a| {
            a.pointer("/result/applied").and_then(serde_json::Value::as_bool) == Some(true)
                && a.pointer("/result/verification_error").is_none()
                && a.pointer("/result/readback").is_some()
        })
        .count();
    let text = if verified > 0 {
        format!("{verified} workbook operation completed and read back. Review the cells and sources.")
    } else if view.status == "complete" {
        "The request finished. No verified workbook edits were made. Review the response and sources.".into()
    } else {
        return Err(ApiError(StatusCode::CONFLICT, "verified_outcome_not_available"));
    };
    let bytes = client.synthesize(&text).await.map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "synthesis_failed"))?;
    Ok(([(header::CONTENT_TYPE, "audio/mpeg"), (header::CACHE_CONTROL, "no-store")], bytes).into_response())
}
