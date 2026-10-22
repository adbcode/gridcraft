//! Real PostgreSQL transaction tests, deliberately excluded from the provider-free suite.
//! Run only against an explicitly supplied disposable GRIDCRAFT_TEST_DATABASE_URL.
//! Each test owns random workspace UUIDs; no database-wide truncate/drop is performed.

use gridcraft_engine::agent_bridge::Identity;
use gridcraft_engine::review_workflow::{ComparisonPeriods, ReviewError, demo_sales};
use gridcraft_server::storage::{SnapshotMetadata, SnapshotUpload, StorageError, Store, migrate};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn database() -> Result<PgPool, Box<dyn std::error::Error>> {
    let url = std::env::var("GRIDCRAFT_TEST_DATABASE_URL")
        .map_err(|_| "GRIDCRAFT_TEST_DATABASE_URL must explicitly identify a disposable PostgreSQL database")?;
    let pool = PgPoolOptions::new().max_connections(5).connect(&url).await.map_err(|_| StorageError::Database)?;
    migrate(&pool).await?;
    Ok(pool)
}

async fn cleanup(pool: &PgPool, workspace_id: Uuid) -> Result<(), StorageError> {
    for query in [
        "DELETE FROM review_snapshots WHERE workspace_id=$1",
        "DELETE FROM review_workbooks WHERE workspace_id=$1",
        "DELETE FROM review_queries WHERE workspace_id=$1",
        "DELETE FROM review_rules WHERE workspace_id=$1",
        "DELETE FROM review_sales WHERE workspace_id=$1",
        "DELETE FROM review_workspaces WHERE workspace_id=$1",
    ] {
        sqlx::query(query).bind(workspace_id).execute(pool).await?;
    }
    Ok(())
}

fn upload(workbook_id: Uuid, export_id: &str, expected_version: i64, contents: &[u8]) -> SnapshotUpload {
    SnapshotUpload {
        workbook_id,
        metadata: SnapshotMetadata {
            export_id: export_id.into(),
            workbook_session_id: "browser-session-1".into(),
            identity: Identity { document_id: 7, sheet_index: 0, sheet_name: "Sales".into(), change_token: 12 },
            expected_version,
        },
        // Persistence deliberately treats bytes as an opaque exported file. The browser
        // exporter/importer owns workbook format validation, tested in the engine suite.
        xlsx: contents.into(),
    }
}

#[tokio::test]
#[ignore = "requires an explicit disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn rules_fixture_and_query_evidence_survive_pool_restart() -> TestResult {
    let pool = database().await?;
    let workspace = Uuid::new_v4();
    let store = Store::new(pool.clone(), workspace);
    store.check_ready().await?;
    assert!(store.list_rules().await?.is_empty());
    assert_eq!(store.seed_demo().await?, 12);
    assert_eq!(store.seed_demo().await?, 12);
    assert_eq!(store.compare(ComparisonPeriods::demo()).await?.total().current_minor, 65_500);
    assert!(matches!(store.save_rule("unsupported", "ignore small orders").await, Err(StorageError::Review(ReviewError::UnsupportedRule))));
    assert!(store.list_rules().await?.is_empty());
    let rule = store.save_rule("exclude-tests", "Remember to exclude test orders").await?;
    assert_eq!(store.save_rule("exclude-tests", "Remember to exclude test orders").await?, rule);
    assert!(matches!(store.save_rule("exclude-tests", "exclude test orders").await, Err(StorageError::Conflict)));
    let result = store.compare(ComparisonPeriods::demo()).await?;
    assert_eq!(result.total().previous_minor, 15_000);
    assert_eq!(result.total().current_minor, 15_500);
    assert_eq!(result.provenance().source_record_count, 12);
    assert_eq!(result.provenance().previous_record_count, 4);
    assert_eq!(result.provenance().current_record_count, 3);
    assert_eq!(result.provenance().applied_rule_ids, ["exclude-tests"]);
    let jackets = result.categories().iter().find(|category| category.category == "Jackets").ok_or("missing Jackets fixture category")?;
    assert_eq!(jackets.delta_minor, -4_000);
    assert_eq!(jackets.growth, Some(-0.4));
    let evidence: serde_json::Value = sqlx::query_scalar("SELECT result FROM review_queries WHERE workspace_id=$1 AND query_id=$2")
        .bind(workspace)
        .bind(Uuid::parse_str(&result.provenance().query_id)?)
        .fetch_one(&pool)
        .await
        .map_err(|_| StorageError::Database)?;
    assert_eq!(evidence, serde_json::to_value(&result)?);
    drop(store);
    pool.close().await;

    let restarted_pool = database().await?;
    let restarted_store = Store::new(restarted_pool.clone(), workspace);
    assert_eq!(restarted_store.list_rules().await?, [rule]);
    assert_eq!(restarted_store.compare(ComparisonPeriods::demo()).await?.total().current_minor, 15_500);
    cleanup(&restarted_pool, workspace).await?;
    restarted_pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an explicit disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn snapshots_serialize_concurrent_versions_and_recover_exact_receipts() -> TestResult {
    let pool = database().await?;
    let workspace = Uuid::new_v4();
    let store = Store::new(pool.clone(), workspace);
    let workbook_id = Uuid::new_v4();
    store.create_workbook(workbook_id, "Weekly review").await?;
    let (left, right) = tokio::join!(
        store.save_snapshot(upload(workbook_id, "first-left", 0, b"left")),
        store.save_snapshot(upload(workbook_id, "first-right", 0, b"right")),
    );
    let (first, bytes) = match (left, right) {
        (Ok(receipt), Err(StorageError::Conflict)) => (receipt, b"left".as_slice()),
        (Err(StorageError::Conflict), Ok(receipt)) => (receipt, b"right".as_slice()),
        _ => return Err("concurrent saves must commit exactly one expected version".into()),
    };
    assert_eq!(first.storage_version, 1);
    assert_eq!(first.sha256.len(), 64);
    assert_eq!(store.list_workbooks().await?.first().map(|workbook| workbook.latest_version), Some(1));

    let second = store.save_snapshot(upload(workbook_id, "second", 1, b"later")).await?;
    assert_eq!(second.storage_version, 2);
    let recovered = store.save_snapshot(upload(workbook_id, &first.export_id, 0, bytes)).await?;
    assert_eq!(recovered, first);
    assert!(matches!(store.save_snapshot(upload(workbook_id, &first.export_id, 1, bytes)).await, Err(StorageError::Conflict)));
    assert!(matches!(store.save_snapshot(upload(workbook_id, &first.export_id, 0, b"different bytes")).await, Err(StorageError::Conflict)));
    let mut changed_identity = upload(workbook_id, &first.export_id, 0, bytes);
    changed_identity.metadata.identity.change_token += 1;
    assert!(matches!(store.save_snapshot(changed_identity).await, Err(StorageError::Conflict)));
    let mut wrong_session = upload(workbook_id, &first.export_id, 0, bytes);
    wrong_session.metadata.workbook_session_id = "another-session".into();
    assert!(matches!(store.save_snapshot(wrong_session).await, Err(StorageError::Conflict)));

    let (retry_a, retry_b) = tokio::join!(
        store.save_snapshot(upload(workbook_id, "third", 2, b"same concurrent request")),
        store.save_snapshot(upload(workbook_id, "third", 2, b"same concurrent request")),
    );
    let third = retry_a?;
    assert_eq!(third, retry_b?);
    assert_eq!(third.storage_version, 3);
    drop(store);
    pool.close().await;

    let restarted_pool = database().await?;
    let restarted_store = Store::new(restarted_pool.clone(), workspace);
    let saved = restarted_store.latest_snapshot(workbook_id).await?;
    assert_eq!(saved.receipt, third);
    assert_eq!(saved.xlsx, b"same concurrent request");
    assert_eq!(restarted_store.save_snapshot(upload(workbook_id, "third", 2, b"same concurrent request")).await?, third);
    cleanup(&restarted_pool, workspace).await?;
    restarted_pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an explicit disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn workspace_boundaries_and_seed_conflicts_are_transactional() -> TestResult {
    let pool = database().await?;
    let owner = Uuid::new_v4();
    let other_owner = Uuid::new_v4();
    let store = Store::new(pool.clone(), owner);
    let other = Store::new(pool.clone(), other_owner);
    let workbook_id = Uuid::new_v4();
    store.create_workbook(workbook_id, "Private workbook").await?;
    store.save_snapshot(upload(workbook_id, "one", 0, b"private bytes")).await?;
    store.save_rule("remembered", "exclude test orders").await?;
    assert!(other.list_workbooks().await?.is_empty());
    assert!(other.list_rules().await?.is_empty());
    assert!(matches!(other.latest_snapshot(workbook_id).await, Err(StorageError::NotFound)));
    assert!(matches!(other.save_snapshot(upload(workbook_id, "one", 0, b"attack")).await, Err(StorageError::NotFound)));
    other.create_workbook(workbook_id, "Other owner same ID").await?;
    other.save_snapshot(upload(workbook_id, "one", 0, b"other bytes")).await?;
    assert_eq!(store.latest_snapshot(workbook_id).await?.xlsx, b"private bytes");
    assert_eq!(other.latest_snapshot(workbook_id).await?.xlsx, b"other bytes");

    store.seed_demo().await?;
    assert_eq!(other.compare(ComparisonPeriods::demo()).await?.total().current_minor, 0);
    sqlx::query("DELETE FROM review_sales WHERE workspace_id=$1 AND record_id='p-jackets-a'")
        .bind(owner)
        .execute(&pool)
        .await
        .map_err(|_| StorageError::Database)?;
    sqlx::query("UPDATE review_sales SET category='Changed manually' WHERE workspace_id=$1 AND record_id='p-jackets-b'")
        .bind(owner)
        .execute(&pool)
        .await
        .map_err(|_| StorageError::Database)?;
    assert!(matches!(store.seed_demo().await, Err(StorageError::Conflict)));
    let restored: i64 = sqlx::query_scalar("SELECT count(*) FROM review_sales WHERE workspace_id=$1 AND record_id='p-jackets-a'")
        .bind(owner)
        .fetch_one(&pool)
        .await
        .map_err(|_| StorageError::Database)?;
    assert_eq!(restored, 0, "a failed seed must roll back rows inserted before the conflict");
    cleanup(&pool, owner).await?;
    cleanup(&pool, other_owner).await?;
    pool.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an explicit disposable GRIDCRAFT_TEST_DATABASE_URL"]
async fn changing_sources_update_fresh_answers_and_preserve_exact_prior_evidence() -> TestResult {
    let pool = database().await?;
    let owner = Uuid::new_v4();
    let other_owner = Uuid::new_v4();
    let store = Store::new(pool.clone(), owner);
    store.seed_demo().await?;
    store.save_rule("exclude-tests", "exclude test orders").await?;
    let before = store.compare(ComparisonPeriods::demo()).await?;
    let before_id = Uuid::parse_str(&before.provenance().query_id)?;
    let evidence_before = store.query_evidence(before_id).await?;
    assert_eq!(evidence_before.source_revision, 1);
    assert_eq!(evidence_before.source_records, demo_sales_sorted());
    assert!(evidence_before.source_origins.values().all(|origin| origin == "synthetic_demo"));
    assert!(evidence_before.queried_at_utc.ends_with('Z'));
    assert_eq!(evidence_before.result, serde_json::to_value(&before)?);

    let mut changed = demo_sales().into_iter().find(|sale| sale.id == "c-jackets").ok_or("missing fixture sale")?;
    changed.amount_minor = 10_000;
    let imported = store.upsert_sales(&[changed.clone()]).await?;
    assert_eq!(imported.submitted, 1);
    assert_eq!(imported.changed, 1);
    assert_eq!(imported.source_revision, 2);
    let repeated = store.upsert_sales(&[changed.clone()]).await?;
    assert_eq!(repeated.changed, 0);
    assert_eq!(repeated.source_revision, 2);

    // A duplicate record ID or an invalid record rejects the complete batch, without
    // partially persisting otherwise-valid changes or advancing its freshness token.
    let mut second_change = changed.clone();
    second_change.amount_minor = 12_000;
    assert!(matches!(store.upsert_sales(&[second_change.clone(), second_change.clone()]).await, Err(StorageError::Review(_))));
    let mut invalid = demo_sales().into_iter().find(|sale| sale.id == "c-shoes").ok_or("missing fixture sale")?;
    invalid.refunded_minor = invalid.amount_minor.saturating_add(1);
    assert!(matches!(store.upsert_sales(&[second_change, invalid]).await, Err(StorageError::Review(_))));

    let reloaded = Store::new(pool.clone(), owner);
    assert_eq!(reloaded.list_rules().await?.len(), 1);
    let after = reloaded.compare(ComparisonPeriods::demo()).await?;
    assert_eq!(after.total().current_minor, 18_500);
    let evidence_after = reloaded.query_evidence(Uuid::parse_str(&after.provenance().query_id)?).await?;
    assert_eq!(evidence_after.source_revision, 2);
    assert_eq!(evidence_after.source_origins.get("c-jackets").map(String::as_str), Some("imported"));
    assert_eq!(evidence_after.source_records.iter().find(|sale| sale.id == "c-jackets"), Some(&changed));
    assert_eq!(serde_json::to_value(reloaded.query_evidence(before_id).await?)?, serde_json::to_value(evidence_before)?);
    let other = Store::new(pool.clone(), other_owner);
    assert!(matches!(other.query_evidence(before_id).await, Err(StorageError::NotFound)));
    cleanup(&pool, owner).await?;
    cleanup(&pool, other_owner).await?;
    pool.close().await;
    Ok(())
}

fn demo_sales_sorted() -> Vec<gridcraft_engine::review_workflow::Sale> {
    let mut sales = demo_sales();
    sales.sort_by(|a, b| a.id.cmp(&b.id));
    sales
}
