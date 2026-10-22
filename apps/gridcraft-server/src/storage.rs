//! PostgreSQL persistence for one server-selected workspace. All SQL is parameterized;
//! browser identities and database versions remain separate. No database error, URL or
//! query text is returned to clients or included in this module's error messages.

use std::{collections::BTreeMap, fmt, time::Duration};

use gridcraft_engine::agent_bridge::Identity;
use gridcraft_engine::review_workflow::{
    ComparisonPeriods, Currency, MAX_RECORDS, MAX_RULES, ReviewError, ReviewRule, Sale, SaleStatus, SalesReview, compare_sales, demo_sales,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool, Postgres, QueryBuilder, Row, Transaction, migrate::MigrateError, postgres::PgRow};
use uuid::Uuid;

pub const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_IMPORT_RECORDS: usize = 1_000;
const MAX_WORKBOOKS: i64 = 1_000;
const MAX_IDENTITY_TOKEN: u64 = 9_007_199_254_740_991;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("record was not found in this workspace")]
    NotFound,
    #[error("request conflicts with the stored operation or current database version")]
    Conflict,
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    #[error("storage request exceeds the supported limits")]
    LimitExceeded,
    #[error(transparent)]
    Review(#[from] ReviewError),
    #[error("database operation failed")]
    Database,
    #[error("database migration failed")]
    Migration,
    #[error("stored data does not match the supported contract")]
    CorruptData,
}

type Result<T> = std::result::Result<T, StorageError>;

impl From<sqlx::Error> for StorageError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}

/// Startup-only diagnostics: never retain driver messages, SQL, identifiers or credentials.
#[derive(Debug, thiserror::Error)]
pub struct MigrationError {
    phase: &'static str,
    reason: &'static str,
    version: Option<i64>,
    sqlstate: Option<String>,
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "database migration failed during {}: {}", self.phase, self.reason)?;
        if let Some(version) = self.version {
            write!(f, " (migration {version})")?;
        }
        if let Some(code) = &self.sqlstate {
            write!(f, " (SQLSTATE {code})")?;
        }
        Ok(())
    }
}

impl MigrationError {
    fn new(phase: &'static str, reason: &'static str) -> Self {
        Self { phase, reason, version: None, sqlstate: None }
    }

    fn database(phase: &'static str, error: sqlx::Error) -> Self {
        let reason = match &error {
            sqlx::Error::Tls(_) => "TLS verification failed; check the PostgreSQL hostname and project CA PEM (keep verify-full)",
            sqlx::Error::Io(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                "TLS/protocol negotiation failed; check the PostgreSQL hostname and project CA PEM (keep verify-full)"
            }
            sqlx::Error::Io(_) => "network connection failed; check the PostgreSQL host, port, service status and network access",
            sqlx::Error::Configuration(_) => "invalid database connection configuration; check DATABASE_URL and CA settings",
            sqlx::Error::PoolTimedOut => "connection timed out; check PostgreSQL connectivity and service limits",
            sqlx::Error::Database(_) => "PostgreSQL rejected the operation",
            _ => "database driver failed; check connection settings and migration compatibility",
        };
        let mut diagnostic = Self::new(phase, reason);
        if let Some(code) = error.as_database_error().and_then(|error| error.code()) {
            diagnostic.set_sqlstate(&code);
        }
        diagnostic
    }

    fn set_sqlstate(&mut self, code: &str) {
        if code.len() != 5 || !code.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()) {
            return;
        }
        self.sqlstate = Some(code.to_owned());
        self.reason = match code {
            "28P01" | "28000" => "authentication rejected; check the PostgreSQL user/password and access policy in DATABASE_URL",
            "3D000" => "database does not exist; check the database name in DATABASE_URL",
            "42501" => "insufficient privileges; the migration user needs schema/table creation and migration-lock access",
            "42P07" | "42710" => "schema object already exists; inspect existing schema and migration history before making changes",
            "3F000" => "target schema does not exist; check the connection search_path",
            "53300" => "PostgreSQL connection limit reached; check service capacity and connection usage",
            "57P03" => "PostgreSQL is not accepting connections; check service status",
            _ => self.reason,
        };
    }

    fn migration(error: MigrateError) -> Self {
        let (reason, version) = match error {
            MigrateError::Execute(error) => return Self::database("migration setup/locking", error),
            MigrateError::ExecuteMigration(error, version) => {
                let mut diagnostic = Self::database("migration SQL", error);
                diagnostic.version = Some(version);
                return diagnostic;
            }
            MigrateError::VersionMismatch(version) => {
                ("applied migration checksum differs from this build; restore the original migration, do not delete history", Some(version))
            }
            MigrateError::VersionMissing(version) => {
                ("database contains a migration missing from this build; deploy the matching migration history", Some(version))
            }
            MigrateError::Dirty(version) => ("a migration is marked incomplete; inspect and repair it before restarting", Some(version)),
            _ => ("migration history or source is incompatible with this build", None),
        };
        Self { version, ..Self::new("migration validation", reason) }
    }
}

pub async fn migrate(pool: &PgPool) -> std::result::Result<(), MigrationError> {
    // A direct connection preserves failures that pool retries can mask as timeouts.
    // Use the same TLS policy, and close on return to release even a failed migration's lock.
    let mut connection = tokio::time::timeout(Duration::from_secs(10), PgConnection::connect_with(&pool.connect_options()))
        .await
        .map_err(|_| MigrationError::new("connection", "connection timed out; check PostgreSQL connectivity and service limits"))?
        .map_err(|error| MigrationError::database("connection", error))?;
    sqlx::migrate!("./migrations").run(&mut connection).await.map_err(MigrationError::migration)
}

#[derive(Clone)]
pub struct Store {
    pub(crate) pool: PgPool,
    pub(crate) workspace_id: Uuid,
    pub(crate) run_gate: std::sync::Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workbook {
    pub workbook_id: Uuid,
    pub title: String,
    pub latest_version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotMetadata {
    pub export_id: String,
    pub workbook_session_id: String,
    pub identity: Identity,
    pub expected_version: i64,
}

pub struct SnapshotUpload {
    pub workbook_id: Uuid,
    pub metadata: SnapshotMetadata,
    pub xlsx: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotReceipt {
    pub workbook_id: Uuid,
    pub export_id: String,
    pub workbook_session_id: String,
    pub identity: Identity,
    pub storage_version: i64,
    pub sha256: String,
    pub byte_length: usize,
}

pub struct StoredSnapshot {
    pub receipt: SnapshotReceipt,
    pub xlsx: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SalesImportReceipt {
    pub submitted: usize,
    pub changed: usize,
    pub source_revision: i64,
}

/// Immutable evidence from the same database snapshot as the displayed comparison.
/// Origins describe how each record entered this application, not an independently
/// verified claim that an imported record came from a business system.
#[derive(Clone, Debug, Serialize)]
pub struct QueryEvidence {
    pub query_id: Uuid,
    pub source_revision: i64,
    pub queried_at_utc: String,
    pub provenance: serde_json::Value,
    pub result: serde_json::Value,
    pub source_records: Vec<Sale>,
    pub rule_context: Vec<ReviewRule>,
    pub source_origins: BTreeMap<String, String>,
}

impl Store {
    pub fn new(pool: PgPool, workspace_id: Uuid) -> Self {
        Self { pool, workspace_id, run_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())) }
    }

    /// A connection alone is insufficient: readiness also requires the executable
    /// migration and the columns used by this server. This does not probe any provider.
    pub async fn check_ready(&self) -> Result<()> {
        let _: i32 = sqlx::query_scalar("SELECT 1").fetch_one(&self.pool).await?;
        let migrated: bool =
            sqlx::query_scalar("SELECT count(*)=2 FROM _sqlx_migrations WHERE version IN (1,3) AND success").fetch_one(&self.pool).await?;
        if !migrated {
            return Err(StorageError::Migration);
        }
        sqlx::query(
            "SELECT w.workspace_id,w.source_revision,s.timestamp_utc,s.origin,r.kind,q.provenance,q.source_records,b.latest_version,f.expected_version,f.exported_identity
             FROM review_workspaces w,review_sales s,review_rules r,review_queries q,review_workbooks b,review_snapshots f LIMIT 0",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("SELECT state FROM review_agent_runs LIMIT 0").execute(&self.pool).await?;
        Ok(())
    }

    /// Explicitly seeds only the original synthetic source records, never remembered
    /// rules. Repeating an identical seed is harmless; existing conflicting data aborts
    /// the whole transaction instead of being overwritten.
    pub async fn seed_demo(&self) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        self.lock_workspace(&mut tx).await?;
        let fixture = demo_sales();
        let mut changed = 0_u64;
        for sale in &fixture {
            let inserted = sqlx::query(
                "INSERT INTO review_sales
                 (workspace_id, record_id, category, timestamp_utc, amount_minor, refunded_minor, currency, status, is_test,origin)
                 VALUES ($1,$2,$3,$4,$5,$6,'EUR',$7,$8,'synthetic_demo') ON CONFLICT (workspace_id, record_id) DO NOTHING",
            )
            .bind(self.workspace_id)
            .bind(&sale.id)
            .bind(&sale.category)
            .bind(sale.timestamp_utc)
            .bind(sale.amount_minor)
            .bind(sale.refunded_minor)
            .bind(status_text(sale.status))
            .bind(sale.is_test)
            .execute(&mut *tx)
            .await?;
            changed = changed.saturating_add(inserted.rows_affected());
            let row = sqlx::query("SELECT * FROM review_sales WHERE workspace_id=$1 AND record_id=$2")
                .bind(self.workspace_id)
                .bind(&sale.id)
                .fetch_one(&mut *tx)
                .await?;
            if sale_from_row(&row)? != *sale {
                return Err(StorageError::Conflict);
            }
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM review_sales WHERE workspace_id=$1").bind(self.workspace_id).fetch_one(&mut *tx).await?;
        if count > MAX_RECORDS as i64 {
            return Err(StorageError::LimitExceeded);
        }
        if changed > 0 {
            self.advance_source_revision(&mut tx).await?;
        }
        tx.commit().await?;
        Ok(fixture.len())
    }

    /// Import typed source data atomically. The caller cannot choose a workspace or issue
    /// SQL. Identical records do not change revision/origin; changed records are marked as
    /// imported. The revision advances once per successful batch that changes any data.
    pub async fn upsert_sales(&self, records: &[Sale]) -> Result<SalesImportReceipt> {
        if records.is_empty() || records.len() > MAX_IMPORT_RECORDS {
            return Err(StorageError::LimitExceeded);
        }
        // The engine validates every source record, including records outside these
        // comparison dates, and rejects duplicate IDs before any database work.
        compare_sales(records, &[], "validate-import", ComparisonPeriods::demo())?;
        let mut tx = self.pool.begin().await?;
        self.lock_workspace(&mut tx).await?;
        // One parameterized statement avoids a database round trip for every record.
        // At most 10,000 bindings are emitted, well below PostgreSQL's parameter limit.
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO review_sales
             (workspace_id,record_id,category,timestamp_utc,amount_minor,refunded_minor,currency,status,is_test,origin) ",
        );
        query.push_values(records, |mut row, sale| {
            row.push_bind(self.workspace_id)
                .push_bind(&sale.id)
                .push_bind(&sale.category)
                .push_bind(sale.timestamp_utc)
                .push_bind(sale.amount_minor)
                .push_bind(sale.refunded_minor)
                .push_bind("EUR")
                .push_bind(status_text(sale.status))
                .push_bind(sale.is_test)
                .push_bind("imported");
        });
        query.push(
            " ON CONFLICT (workspace_id,record_id) DO UPDATE SET
               category=EXCLUDED.category,timestamp_utc=EXCLUDED.timestamp_utc,
               amount_minor=EXCLUDED.amount_minor,refunded_minor=EXCLUDED.refunded_minor,
               currency=EXCLUDED.currency,status=EXCLUDED.status,is_test=EXCLUDED.is_test,origin='imported'
             WHERE (review_sales.category,review_sales.timestamp_utc,review_sales.amount_minor,
                    review_sales.refunded_minor,review_sales.currency,review_sales.status,review_sales.is_test)
               IS DISTINCT FROM (EXCLUDED.category,EXCLUDED.timestamp_utc,EXCLUDED.amount_minor,
                                 EXCLUDED.refunded_minor,EXCLUDED.currency,EXCLUDED.status,EXCLUDED.is_test)",
        );
        let affected = query.build().execute(&mut *tx).await?;
        let changed = usize::try_from(affected.rows_affected()).map_err(|_| StorageError::LimitExceeded)?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM review_sales WHERE workspace_id=$1").bind(self.workspace_id).fetch_one(&mut *tx).await?;
        if count > MAX_RECORDS as i64 {
            return Err(StorageError::LimitExceeded);
        }
        let source_revision = if changed > 0 {
            self.advance_source_revision(&mut tx).await?
        } else {
            sqlx::query_scalar("SELECT source_revision FROM review_workspaces WHERE workspace_id=$1")
                .bind(self.workspace_id)
                .fetch_one(&mut *tx)
                .await?
        };
        tx.commit().await?;
        Ok(SalesImportReceipt { submitted: records.len(), changed, source_revision })
    }

    pub async fn list_rules(&self) -> Result<Vec<ReviewRule>> {
        let mut tx = self.pool.begin().await?;
        let rules = self.read_rules(&mut tx).await?;
        tx.commit().await?;
        Ok(rules)
    }

    /// An existing rule ID is an immutable create-operation ID. Identical retries return
    /// the original rule; different wording under the same ID is rejected.
    pub async fn save_rule(&self, id: &str, wording: &str) -> Result<ReviewRule> {
        let rule = ReviewRule::from_instruction(id, wording)?;
        let mut tx = self.pool.begin().await?;
        self.lock_workspace(&mut tx).await?;
        if let Some(row) = sqlx::query("SELECT * FROM review_rules WHERE workspace_id=$1 AND rule_id=$2")
            .bind(self.workspace_id)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
        {
            let stored = rule_from_row(&row)?;
            if stored != rule {
                return Err(StorageError::Conflict);
            }
            tx.commit().await?;
            return Ok(stored);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM review_rules WHERE workspace_id=$1").bind(self.workspace_id).fetch_one(&mut *tx).await?;
        if count >= MAX_RULES as i64 {
            return Err(StorageError::LimitExceeded);
        }
        sqlx::query("INSERT INTO review_rules (workspace_id,rule_id,original_wording,kind) VALUES ($1,$2,$3,'exclude_test_orders')")
            .bind(self.workspace_id)
            .bind(id)
            .bind(wording)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(rule)
    }

    /// Source rows and rules are read from a single repeatable-read database snapshot.
    /// The existing pure engine query applies the supported structured rule. An over-limit
    /// source set fails rather than producing a silently truncated aggregate. Full source
    /// counts include records excluded by date/status/rules, matching the fixture contract.
    pub async fn compare(&self, periods: ComparisonPeriods) -> Result<SalesReview> {
        self.compare_with_id(periods, Uuid::new_v4()).await
    }

    /// A run tool's stable query ID makes a crash after evidence commit recoverable.
    pub async fn compare_with_id(&self, periods: ComparisonPeriods, query_id: Uuid) -> Result<SalesReview> {
        match self.query_evidence(query_id).await {
            Ok(evidence) => {
                let review = compare_sales(&evidence.source_records, &evidence.rule_context, &query_id.to_string(), periods)?;
                if serde_json::to_value(&review).map_err(|_| StorageError::CorruptData)? != evidence.result {
                    return Err(StorageError::Conflict);
                }
                if review.provenance().periods != periods {
                    return Err(StorageError::Conflict);
                }
                return Ok(review);
            }
            Err(StorageError::NotFound) => {}
            Err(error) => return Err(error),
        }
        // Validate periods before any database work.
        compare_sales(&[], &[], "validate", periods)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").execute(&mut *tx).await?;
        let source_revision: i64 = sqlx::query_scalar("SELECT source_revision FROM review_workspaces WHERE workspace_id=$1")
            .bind(self.workspace_id)
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or(0);
        let rules = self.read_rules(&mut tx).await?;
        let rows = sqlx::query("SELECT * FROM review_sales WHERE workspace_id=$1 ORDER BY record_id LIMIT $2")
            .bind(self.workspace_id)
            .bind((MAX_RECORDS as i64).saturating_add(1))
            .fetch_all(&mut *tx)
            .await?;
        if rows.len() > MAX_RECORDS {
            return Err(StorageError::LimitExceeded);
        }
        let records = rows.iter().map(sale_from_row).collect::<Result<Vec<_>>>()?;
        let source_origins = rows
            .iter()
            .map(|row| Ok((row.try_get::<String, _>("record_id")?, row.try_get::<String, _>("origin")?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let result = compare_sales(&records, &rules, &query_id.to_string(), periods)?;
        // A first query of an empty workspace is valid and still has durable provenance.
        sqlx::query("INSERT INTO review_workspaces (workspace_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(self.workspace_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO review_queries (workspace_id,query_id,provenance,result,rule_context,source_revision,source_records,source_origins)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(self.workspace_id)
        .bind(query_id)
        .bind(serde_json::to_value(result.provenance()).map_err(|_| StorageError::CorruptData)?)
        .bind(serde_json::to_value(&result).map_err(|_| StorageError::CorruptData)?)
        .bind(serde_json::to_value(&rules).map_err(|_| StorageError::CorruptData)?)
        .bind(source_revision)
        .bind(serde_json::to_value(&records).map_err(|_| StorageError::CorruptData)?)
        .bind(serde_json::to_value(&source_origins).map_err(|_| StorageError::CorruptData)?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn query_evidence(&self, query_id: Uuid) -> Result<QueryEvidence> {
        let row = sqlx::query(
            "SELECT *,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS queried_at_utc
             FROM review_queries WHERE workspace_id=$1 AND query_id=$2",
        )
        .bind(self.workspace_id)
        .bind(query_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        let source_records = serde_json::from_value(row.try_get::<serde_json::Value, _>("source_records")?).map_err(|_| StorageError::CorruptData)?;
        let rule_context = rules_from_json(row.try_get("rule_context")?)?;
        let source_origins = serde_json::from_value(row.try_get::<serde_json::Value, _>("source_origins")?).map_err(|_| StorageError::CorruptData)?;
        Ok(QueryEvidence {
            query_id,
            source_revision: row.try_get("source_revision")?,
            queried_at_utc: row.try_get("queried_at_utc")?,
            provenance: row.try_get("provenance")?,
            result: row.try_get("result")?,
            source_records,
            rule_context,
            source_origins,
        })
    }

    pub async fn create_workbook(&self, id: Uuid, title: &str) -> Result<Workbook> {
        validate_text(title, "workbook title")?;
        let mut tx = self.pool.begin().await?;
        self.lock_workspace(&mut tx).await?;
        if let Some(row) = sqlx::query("SELECT workbook_id,title,latest_version FROM review_workbooks WHERE workspace_id=$1 AND workbook_id=$2")
            .bind(self.workspace_id)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
        {
            let workbook = workbook_from_row(&row)?;
            if workbook.title != title {
                return Err(StorageError::Conflict);
            }
            tx.commit().await?;
            return Ok(workbook);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM review_workbooks WHERE workspace_id=$1").bind(self.workspace_id).fetch_one(&mut *tx).await?;
        if count >= MAX_WORKBOOKS {
            return Err(StorageError::LimitExceeded);
        }
        sqlx::query("INSERT INTO review_workbooks (workspace_id,workbook_id,title) VALUES ($1,$2,$3)")
            .bind(self.workspace_id)
            .bind(id)
            .bind(title)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Workbook { workbook_id: id, title: title.into(), latest_version: 0 })
    }

    pub async fn list_workbooks(&self) -> Result<Vec<Workbook>> {
        let rows = sqlx::query(
            "SELECT workbook_id,title,latest_version FROM review_workbooks WHERE workspace_id=$1 ORDER BY created_at,workbook_id LIMIT $2",
        )
        .bind(self.workspace_id)
        .bind(MAX_WORKBOOKS.saturating_add(1))
        .fetch_all(&self.pool)
        .await?;
        if rows.len() > MAX_WORKBOOKS as usize {
            return Err(StorageError::LimitExceeded);
        }
        rows.iter().map(workbook_from_row).collect()
    }

    /// The workbook row serializes version allocation. Exact duplicate lookup deliberately
    /// precedes the current-version check, so a completed save can be recovered after
    /// another later save. Bytes and all original preconditions must still match.
    pub async fn save_snapshot(&self, upload: SnapshotUpload) -> Result<SnapshotReceipt> {
        validate_snapshot(&upload)?;
        let digest = Sha256::digest(&upload.xlsx).to_vec();
        let identity = serde_json::to_value(&upload.metadata.identity).map_err(|_| StorageError::CorruptData)?;
        let mut tx = self.pool.begin().await?;
        let current: i64 = sqlx::query_scalar("SELECT latest_version FROM review_workbooks WHERE workspace_id=$1 AND workbook_id=$2 FOR UPDATE")
            .bind(self.workspace_id)
            .bind(upload.workbook_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::NotFound)?;
        if let Some(row) =
            sqlx::query("SELECT * FROM review_snapshots WHERE workspace_id=$1 AND workbook_id=$2 AND workbook_session_id=$3 AND export_id=$4")
                .bind(self.workspace_id)
                .bind(upload.workbook_id)
                .bind(&upload.metadata.workbook_session_id)
                .bind(&upload.metadata.export_id)
                .fetch_optional(&mut *tx)
                .await?
        {
            let original_expected: i64 = row.try_get("expected_version")?;
            let saved = snapshot_from_row(&row)?;
            if original_expected != upload.metadata.expected_version
                || saved.receipt.identity != upload.metadata.identity
                || saved.xlsx != upload.xlsx
                || saved.receipt.sha256 != hex_digest(&digest)
            {
                return Err(StorageError::Conflict);
            }
            tx.commit().await?;
            return Ok(saved.receipt);
        }
        if current != upload.metadata.expected_version {
            return Err(StorageError::Conflict);
        }
        let next = current.checked_add(1).ok_or(StorageError::LimitExceeded)?;
        sqlx::query(
            "INSERT INTO review_snapshots
             (workspace_id,workbook_id,snapshot_version,expected_version,export_id,workbook_session_id,exported_identity,xlsx,sha256)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(self.workspace_id)
        .bind(upload.workbook_id)
        .bind(next)
        .bind(upload.metadata.expected_version)
        .bind(&upload.metadata.export_id)
        .bind(&upload.metadata.workbook_session_id)
        .bind(identity)
        .bind(&upload.xlsx)
        .bind(&digest)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE review_workbooks SET latest_version=$3 WHERE workspace_id=$1 AND workbook_id=$2")
            .bind(self.workspace_id)
            .bind(upload.workbook_id)
            .bind(next)
            .execute(&mut *tx)
            .await?;
        let receipt = SnapshotReceipt {
            workbook_id: upload.workbook_id,
            export_id: upload.metadata.export_id,
            workbook_session_id: upload.metadata.workbook_session_id,
            identity: upload.metadata.identity,
            storage_version: next,
            sha256: hex_digest(&digest),
            byte_length: upload.xlsx.len(),
        };
        tx.commit().await?;
        Ok(receipt)
    }

    pub async fn latest_snapshot(&self, id: Uuid) -> Result<StoredSnapshot> {
        let row = sqlx::query(
            "SELECT s.* FROM review_snapshots s JOIN review_workbooks w
             ON s.workspace_id=w.workspace_id AND s.workbook_id=w.workbook_id AND s.snapshot_version=w.latest_version
             WHERE s.workspace_id=$1 AND s.workbook_id=$2",
        )
        .bind(self.workspace_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        snapshot_from_row(&row)
    }

    /// Explicit demo-library initialization. Never overwrite an existing workbook/version.
    pub async fn seed_files(&self) -> Result<()> {
        for (key, title) in gridcraft_engine::workspace::demo_catalog() {
            let id = Uuid::parse_str(key).map_err(|_| StorageError::CorruptData)?;
            let workbook = self.create_workbook(id, title).await?;
            if workbook.latest_version > 0 {
                continue;
            }
            let xlsx = gridcraft_engine::workspace::demo_xlsx(key).map_err(|_| StorageError::CorruptData)?;
            let result = self
                .save_snapshot(SnapshotUpload {
                    workbook_id: id,
                    xlsx,
                    metadata: SnapshotMetadata {
                        export_id: "mock-library-v1".into(),
                        workbook_session_id: "explicit-demo-seed".into(),
                        expected_version: 0,
                        identity: Identity {
                            document_id: 1,
                            sheet_index: 0,
                            sheet_name: if key == gridcraft_engine::workspace::WEEKLY_ID { "Review" } else { "Summary" }.into(),
                            change_token: 1,
                        },
                    },
                })
                .await;
            if !matches!(result, Ok(_) | Err(StorageError::Conflict)) {
                result?;
            }
        }
        Ok(())
    }

    pub async fn snapshot_version(&self, id: Uuid, version: i64) -> Result<StoredSnapshot> {
        let row = sqlx::query("SELECT * FROM review_snapshots WHERE workspace_id=$1 AND workbook_id=$2 AND snapshot_version=$3")
            .bind(self.workspace_id)
            .bind(id)
            .bind(version)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        snapshot_from_row(&row)
    }

    async fn lock_workspace(&self, tx: &mut Transaction<'_, Postgres>) -> Result<()> {
        sqlx::query("INSERT INTO review_workspaces (workspace_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(self.workspace_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("SELECT workspace_id FROM review_workspaces WHERE workspace_id=$1 FOR UPDATE")
            .bind(self.workspace_id)
            .fetch_one(&mut **tx)
            .await?;
        Ok(())
    }

    async fn advance_source_revision(&self, tx: &mut Transaction<'_, Postgres>) -> Result<i64> {
        sqlx::query_scalar(
            "UPDATE review_workspaces SET source_revision=source_revision+1
             WHERE workspace_id=$1 AND source_revision<9223372036854775807 RETURNING source_revision",
        )
        .bind(self.workspace_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(StorageError::LimitExceeded)
    }

    async fn read_rules(&self, tx: &mut Transaction<'_, Postgres>) -> Result<Vec<ReviewRule>> {
        let rows = sqlx::query("SELECT * FROM review_rules WHERE workspace_id=$1 ORDER BY rule_id LIMIT $2")
            .bind(self.workspace_id)
            .bind((MAX_RULES as i64).saturating_add(1))
            .fetch_all(&mut **tx)
            .await?;
        if rows.len() > MAX_RULES {
            return Err(StorageError::LimitExceeded);
        }
        rows.iter().map(rule_from_row).collect()
    }
}

fn validate_text(value: &str, field: &'static str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(StorageError::InvalidInput(field));
    }
    Ok(())
}

fn validate_snapshot(upload: &SnapshotUpload) -> Result<()> {
    validate_text(&upload.metadata.export_id, "export ID")?;
    validate_text(&upload.metadata.workbook_session_id, "workbook session ID")?;
    validate_text(&upload.metadata.identity.sheet_name, "sheet name")?;
    if upload.metadata.expected_version < 0
        || upload.metadata.identity.document_id == 0
        || upload.metadata.identity.document_id > MAX_IDENTITY_TOKEN
        || upload.metadata.identity.change_token == 0
        || upload.metadata.identity.change_token > MAX_IDENTITY_TOKEN
        || upload.metadata.identity.sheet_index > 65_535
    {
        return Err(StorageError::InvalidInput("snapshot identity or expected version"));
    }
    if upload.xlsx.is_empty() || upload.xlsx.len() > MAX_SNAPSHOT_BYTES {
        return Err(StorageError::LimitExceeded);
    }
    Ok(())
}

fn rule_from_row(row: &PgRow) -> Result<ReviewRule> {
    let kind: String = row.try_get("kind")?;
    if kind != "exclude_test_orders" {
        return Err(StorageError::CorruptData);
    }
    let id: String = row.try_get("rule_id")?;
    let wording: String = row.try_get("original_wording")?;
    let mut rule = ReviewRule::from_instruction(&id, &wording).map_err(|_| StorageError::CorruptData)?;
    rule.set_enabled(row.try_get("enabled")?);
    Ok(rule)
}

fn rules_from_json(value: serde_json::Value) -> Result<Vec<ReviewRule>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StoredRule {
        id: String,
        original_wording: String,
        kind: String,
        enabled: bool,
    }
    let stored: Vec<StoredRule> = serde_json::from_value(value).map_err(|_| StorageError::CorruptData)?;
    if stored.len() > MAX_RULES {
        return Err(StorageError::CorruptData);
    }
    stored
        .into_iter()
        .map(|stored| {
            if stored.kind != "exclude_test_orders" {
                return Err(StorageError::CorruptData);
            }
            let mut rule = ReviewRule::from_instruction(&stored.id, &stored.original_wording).map_err(|_| StorageError::CorruptData)?;
            rule.set_enabled(stored.enabled);
            Ok(rule)
        })
        .collect()
}

fn status_text(status: SaleStatus) -> &'static str {
    match status {
        SaleStatus::Paid => "paid",
        SaleStatus::Pending => "pending",
        SaleStatus::Cancelled => "cancelled",
    }
}

fn sale_from_row(row: &PgRow) -> Result<Sale> {
    let status: String = row.try_get("status")?;
    let currency: String = row.try_get("currency")?;
    if currency != "EUR" {
        return Err(StorageError::CorruptData);
    }
    Ok(Sale {
        id: row.try_get("record_id")?,
        category: row.try_get("category")?,
        timestamp_utc: row.try_get("timestamp_utc")?,
        amount_minor: row.try_get("amount_minor")?,
        refunded_minor: row.try_get("refunded_minor")?,
        currency: Currency::Eur,
        status: match status.as_str() {
            "paid" => SaleStatus::Paid,
            "pending" => SaleStatus::Pending,
            "cancelled" => SaleStatus::Cancelled,
            _ => return Err(StorageError::CorruptData),
        },
        is_test: row.try_get("is_test")?,
    })
}

fn workbook_from_row(row: &PgRow) -> Result<Workbook> {
    Ok(Workbook { workbook_id: row.try_get("workbook_id")?, title: row.try_get("title")?, latest_version: row.try_get("latest_version")? })
}

fn snapshot_from_row(row: &PgRow) -> Result<StoredSnapshot> {
    let identity: serde_json::Value = row.try_get("exported_identity")?;
    let xlsx: Vec<u8> = row.try_get("xlsx")?;
    let digest: Vec<u8> = row.try_get("sha256")?;
    if xlsx.is_empty() || xlsx.len() > MAX_SNAPSHOT_BYTES || digest != Sha256::digest(&xlsx).to_vec() {
        return Err(StorageError::CorruptData);
    }
    let receipt = SnapshotReceipt {
        workbook_id: row.try_get("workbook_id")?,
        export_id: row.try_get("export_id")?,
        workbook_session_id: row.try_get("workbook_session_id")?,
        identity: serde_json::from_value(identity).map_err(|_| StorageError::CorruptData)?,
        storage_version: row.try_get("snapshot_version")?,
        sha256: hex_digest(&digest),
        byte_length: xlsx.len(),
    };
    Ok(StoredSnapshot { receipt, xlsx })
}

fn hex_digest(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        // Nibbles are always 0..=15; get keeps this boundary explicit and panic-free.
        if let (Some(high), Some(low)) = (DIGITS.get(usize::from(byte >> 4)), DIGITS.get(usize::from(byte & 15))) {
            result.push(char::from(*high));
            result.push(char::from(*low));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_diagnostics_discard_driver_secrets() {
        for error in [
            sqlx::Error::Tls("private certificate and password".into()),
            sqlx::Error::Configuration("postgres://private-user:private-password@private-host/private-db".into()),
            sqlx::Error::Protocol("private server response".into()),
            sqlx::Error::Io(std::io::Error::other("private network details")),
            sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, "private certificate details")),
        ] {
            let diagnostic = MigrationError::database("connection", error);
            assert!(diagnostic.to_string().contains("database migration failed during connection"));
            assert!(!format!("{diagnostic} {diagnostic:?}").contains("private"));
        }
        let diagnostic = MigrationError::migration(MigrateError::Source("private migration SQL".into()));
        assert!(!format!("{diagnostic} {diagnostic:?}").contains("private"));
        let diagnostic = MigrationError::database("connection", sqlx::Error::Io(std::io::Error::from(std::io::ErrorKind::InvalidData)));
        assert!(diagnostic.to_string().contains("TLS/protocol negotiation failed"));
    }

    #[test]
    fn migration_diagnostics_identify_history_and_sqlstate_without_raw_messages() {
        let diagnostic = MigrationError::migration(MigrateError::VersionMismatch(3));
        assert!(diagnostic.to_string().contains("checksum"));
        assert!(diagnostic.to_string().contains("migration 3"));
        let diagnostic = MigrationError::migration(MigrateError::VersionMissing(2));
        assert!(diagnostic.to_string().contains("missing from this build"));
        assert!(diagnostic.to_string().contains("migration 2"));
        let diagnostic = MigrationError::migration(MigrateError::Dirty(1));
        assert!(diagnostic.to_string().contains("incomplete"));
        for (code, hint) in [("28P01", "authentication"), ("42501", "privileges"), ("42P07", "already exists"), ("3D000", "database does not exist")]
        {
            let mut diagnostic = MigrationError::new("connection", "PostgreSQL rejected the operation");
            diagnostic.set_sqlstate(code);
            assert!(diagnostic.to_string().contains(hint));
            assert!(diagnostic.to_string().contains(code));
        }
        for code in ["private", "42\n07", "42p07", "secret\nlog injection"] {
            let mut diagnostic = MigrationError::new("connection", "PostgreSQL rejected the operation");
            diagnostic.set_sqlstate(code);
            assert!(diagnostic.sqlstate.is_none());
        }
    }

    fn sample_upload() -> SnapshotUpload {
        SnapshotUpload {
            workbook_id: Uuid::nil(),
            metadata: SnapshotMetadata {
                export_id: "export-1".into(),
                workbook_session_id: "session-1".into(),
                identity: Identity { document_id: 1, sheet_index: 0, sheet_name: "Sales".into(), change_token: 1 },
                expected_version: 0,
            },
            xlsx: b"captured bytes".to_vec(),
        }
    }

    #[test]
    fn snapshot_input_is_bounded_before_database_access() {
        let mut upload = sample_upload();
        assert!(validate_snapshot(&upload).is_ok());
        upload.metadata.expected_version = -1;
        assert!(matches!(validate_snapshot(&upload), Err(StorageError::InvalidInput(_))));
        upload.metadata.expected_version = 0;
        upload.metadata.identity.change_token = u64::MAX;
        assert!(matches!(validate_snapshot(&upload), Err(StorageError::InvalidInput(_))));
        upload.metadata.identity.change_token = 1;
        upload.metadata.export_id = "bad\nidentifier".into();
        assert!(matches!(validate_snapshot(&upload), Err(StorageError::InvalidInput(_))));
        upload.metadata.export_id = "export-1".into();
        upload.xlsx.clear();
        assert!(matches!(validate_snapshot(&upload), Err(StorageError::LimitExceeded)));
        upload.xlsx = vec![0; MAX_SNAPSHOT_BYTES.saturating_add(1)];
        assert!(matches!(validate_snapshot(&upload), Err(StorageError::LimitExceeded)));
    }

    #[test]
    fn metadata_rejects_client_selected_owner_and_unknown_fields() {
        let parsed = serde_json::from_value::<SnapshotMetadata>(serde_json::json!({
            "export_id": "export-1",
            "workbook_session_id": "session-1",
            "expected_version": 0,
            "identity": {"document_id":1,"sheet_index":0,"sheet_name":"Sales","change_token":1},
            "workspace_id": Uuid::nil(),
        }));
        assert!(parsed.is_err());
    }

    #[test]
    fn database_errors_never_expose_connection_or_server_details() {
        let error = sqlx::Error::Protocol("postgres://user:secret@example.invalid/database".into());
        let safe = StorageError::from(error);
        assert_eq!(safe.to_string(), "database operation failed");
        assert_eq!(format!("{safe:?}"), "Database");
    }
}
