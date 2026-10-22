//! Speech routes against a local fake ElevenLabs API: bounded input, editable transcript and
//! synthesized audio. No real provider or database is involved.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    routing::post,
};
use gridcraft_server::{
    providers::{ElevenLabsClient, ElevenLabsConfig},
    web::{AppState, MAX_SPOKEN_BYTES, router},
};
use tower::ServiceExt;

async fn app(voice: Option<&str>) -> Result<Router, Box<dyn std::error::Error>> {
    let fake = Router::new()
        .route("/v1/speech-to-text", post(|| async { axum::Json(serde_json::json!({"text": "  Compare sales  ", "language_code": "en"})) }))
        .route("/v1/text-to-speech/{voice}", post(|| async { ([(header::CONTENT_TYPE, "audio/mpeg")], vec![0x49u8, 0x44, 0x33, 1]) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move { axum::serve(listener, fake).await });
    let client = ElevenLabsClient::new(ElevenLabsConfig {
        api_key: "test-key".into(),
        base_url: format!("http://127.0.0.1:{port}/v1"),
        stt_model: "scribe_v2".into(),
        tts_model: "eleven_multilingual_v2".into(),
        voice_id: voice.map(str::to_owned),
    })?;
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("index.html"), "test web asset")?;
    Ok(router(AppState {
        store: None,
        static_dir: directory.keep(),
        openai_configured: false,
        speech_configured: true,
        tts_configured: voice.is_some(),
        runs: None,
        speech: Some(client),
    }))
}

async fn send(app: &Router, path: &str, mime: &str, body: Vec<u8>) -> Result<(StatusCode, Vec<u8>), Box<dyn std::error::Error>> {
    let request = Request::builder().method(Method::POST).uri(path).header(header::CONTENT_TYPE, mime).body(Body::from(body))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    Ok((status, to_bytes(response.into_body(), 1 << 20).await?.to_vec()))
}

#[tokio::test]
async fn transcribe_returns_trimmed_editable_text_and_rejects_bad_audio() -> Result<(), Box<dyn std::error::Error>> {
    let app = app(Some("voice1")).await?;
    let (status, body) = send(&app, "/api/speech/transcribe", "audio/webm;codecs=opus", vec![1, 2, 3]).await?;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(value["text"], "Compare sales");
    assert_eq!(send(&app, "/api/speech/transcribe", "audio/webm", Vec::new()).await?.0, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(send(&app, "/api/speech/transcribe", "text/plain", vec![1]).await?.0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    Ok(())
}

#[tokio::test]
async fn synthesize_is_bounded_and_returns_audio() -> Result<(), Box<dyn std::error::Error>> {
    let app = app(Some("voice1")).await?;
    let (status, body) = send(&app, "/api/speech/synthesize", "application/json", br#"{"text":"Applied and verified."}"#.to_vec()).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, vec![0x49, 0x44, 0x33, 1]);
    let long = serde_json::json!({"text": "x".repeat(MAX_SPOKEN_BYTES + 1)}).to_string().into_bytes();
    assert_eq!(send(&app, "/api/speech/synthesize", "application/json", long).await?.0, StatusCode::UNPROCESSABLE_ENTITY);
    let unvoiced = self::app(None).await?;
    let (status, _) = send(&unvoiced, "/api/speech/synthesize", "application/json", br#"{"text":"Done."}"#.to_vec()).await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}
