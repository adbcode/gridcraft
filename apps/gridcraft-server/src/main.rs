#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use gridcraft_server::{
    config::Config,
    providers::{ElevenLabsClient, OpenAiClient},
    runs::RunService,
    storage::{self, Store},
    web::{self, AppState},
};
use sqlx::postgres::PgPoolOptions;
use std::{process::ExitCode, time::Duration};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("gridcraft-server: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("serve");
    if matches!(command, "--help" | "-h" | "help") {
        println!(
            "gridcraft-server [serve|check-config|migrate|seed-demo]\nReads optional private .env; process environment overrides it. No providers are called at startup."
        );
        return Ok(());
    }
    if args.len() > 1 || !matches!(command, "serve" | "check-config" | "migrate" | "seed-demo") {
        return Err("unknown command; use --help".into());
    }
    let mut config = Config::load()?;
    let openai_configured = config.openai.is_some();
    let speech_configured = config.elevenlabs.is_some();
    let tts_configured = config.elevenlabs.as_ref().is_some_and(|provider| provider.voice_id.is_some());
    // Constructors validate credentials/URLs locally. They do not probe provider accounts.
    let openai = config.openai.take().map(OpenAiClient::new).transpose()?;
    let _elevenlabs = config.elevenlabs.take().map(ElevenLabsClient::new).transpose()?;
    if command == "check-config" {
        println!(
            "Configuration valid. Database configured: {}; OpenAI configured: {openai_configured}; ElevenLabs configured: {speech_configured}. No network checks performed.",
            config.database.is_some()
        );
        return Ok(());
    }
    let pool = config
        .database
        .as_ref()
        .map(|db| {
            db.options().map(|options| PgPoolOptions::new().max_connections(5).acquire_timeout(Duration::from_secs(5)).connect_lazy_with(options))
        })
        .transpose()?;
    let store = pool.as_ref().map(|pool| Store::new(pool.clone(), config.workspace_id));
    if command == "migrate" || config.migrate_on_start {
        storage::migrate(pool.as_ref().ok_or("DATABASE_URL is required for migrations")?).await?;
        println!("Database migrations applied.");
        if command == "migrate" {
            return Ok(());
        }
    }
    if command == "seed-demo" {
        let count = store.as_ref().ok_or("DATABASE_URL is required for demo seeding")?.seed_demo().await?;
        println!("Synthetic sales fixture available: {count} records. No rule was implicitly enabled.");
        return Ok(());
    }
    let runs = store.clone().filter(|_| openai.is_some()).map(|store| RunService::new(store, openai.clone()));
    let state = AppState { store, static_dir: config.static_dir, openai_configured, speech_configured, tts_configured, runs };
    let listener = tokio::net::TcpListener::bind((config.bind, config.port)).await.map_err(|_| "cannot bind configured address/port")?;
    println!("GridCraft service listening on {}:{} (one shared mocked workspace; no login).", config.bind, config.port);
    axum::serve(listener, web::router(state)).with_graceful_shutdown(shutdown()).await.map_err(|_| "HTTP service stopped unexpectedly")?;
    if let Some(pool) = pool {
        pool.close().await;
    }
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
