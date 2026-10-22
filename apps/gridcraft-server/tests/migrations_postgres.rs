//! Opt-in migration diagnostics against random schemas in an explicitly disposable database.

use gridcraft_server::storage::{Store, migrate};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn isolated_database() -> Result<(PgPool, PgPool, String), Box<dyn std::error::Error>> {
    let url = std::env::var("GRIDCRAFT_TEST_DATABASE_URL")?;
    let admin = PgPoolOptions::new().connect(&url).await?;
    let schema = format!("migration_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&admin).await?;
    let options = (*admin.connect_options()).clone().options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new().connect_lazy_with(options);
    Ok((admin, pool, schema))
}

async fn cleanup(admin: PgPool, pool: PgPool, schema: &str) -> TestResult {
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE")).execute(&admin).await?;
    admin.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn migrations_apply_repeat_and_upgrade_without_changing_history() -> TestResult {
    let (admin, pool, schema) = isolated_database().await?;
    let mut baseline = sqlx::migrate!("./migrations");
    baseline.migrations.to_mut().retain(|migration| migration.version == 1);
    baseline.run(&pool).await?;
    migrate(&pool).await?;
    migrate(&pool).await?;
    Store::new(pool.clone(), Uuid::new_v4()).check_ready().await?;
    let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version").fetch_all(&pool).await?;
    assert_eq!(versions, [1, 2]);
    cleanup(admin, pool, &schema).await
}

#[tokio::test]
#[ignore = "requires disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn schema_conflicts_are_actionable_and_retries_release_the_migration_lock() -> TestResult {
    let (admin, pool, schema) = isolated_database().await?;
    sqlx::query("CREATE TABLE review_workspaces (private_column text)").execute(&pool).await?;
    let error = migrate(&pool).await.err().ok_or("expected a migration conflict")?;
    let message = format!("{error} {error:?}");
    assert!(message.contains("migration SQL"));
    assert!(message.contains("migration 1"));
    assert!(message.contains("SQLSTATE 42P07"));
    assert!(!message.contains("review_workspaces"));
    assert!(!message.contains("private_column"));
    sqlx::query("DROP TABLE review_workspaces").execute(&pool).await?;
    tokio::time::timeout(std::time::Duration::from_secs(5), migrate(&pool)).await??;
    cleanup(admin, pool, &schema).await
}

#[tokio::test]
#[ignore = "requires disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn changed_checksums_are_reported_without_rewriting_history() -> TestResult {
    let (admin, pool, schema) = isolated_database().await?;
    migrate(&pool).await?;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=1").execute(&pool).await?;
    let error = migrate(&pool).await.err().ok_or("expected a checksum mismatch")?;
    assert!(error.to_string().contains("checksum"));
    assert!(error.to_string().contains("migration 1"));
    let checksum: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version=1").fetch_one(&pool).await?;
    assert_eq!(checksum, [0]);
    cleanup(admin, pool, &schema).await
}

#[tokio::test]
#[ignore = "requires disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn connection_errors_identify_the_phase_without_exposing_the_database_name() -> TestResult {
    let url = std::env::var("GRIDCRAFT_TEST_DATABASE_URL")?;
    let pool = PgPoolOptions::new().connect_lazy(&url)?;
    let options = (*pool.connect_options()).clone().database("private_missing_database");
    let missing = PgPoolOptions::new().connect_lazy_with(options);
    let error = migrate(&missing).await.err().ok_or("expected missing database")?;
    let message = format!("{error} {error:?}");
    assert!(message.contains("during connection"));
    assert!(message.contains("SQLSTATE 3D000"));
    assert!(!message.contains("private_missing_database"));
    missing.close().await;
    pool.close().await;
    Ok(())
}
