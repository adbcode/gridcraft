-- Executable storage baseline. The workspace is selected by server configuration;
-- there is no client-selectable owner or authentication implementation here.
CREATE TABLE review_workspaces (
    workspace_id uuid PRIMARY KEY,
    source_revision bigint NOT NULL DEFAULT 0 CHECK (source_revision >= 0),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE review_sales (
    workspace_id uuid NOT NULL REFERENCES review_workspaces (workspace_id),
    record_id text NOT NULL CHECK (octet_length(record_id) BETWEEN 1 AND 256),
    category text NOT NULL CHECK (octet_length(category) BETWEEN 1 AND 256),
    timestamp_utc bigint NOT NULL,
    amount_minor bigint NOT NULL CHECK (amount_minor BETWEEN 0 AND 1000000000000),
    refunded_minor bigint NOT NULL CHECK (refunded_minor BETWEEN 0 AND amount_minor),
    currency text NOT NULL CHECK (currency = 'EUR'),
    status text NOT NULL CHECK (status IN ('paid', 'pending', 'cancelled')),
    is_test boolean NOT NULL,
    origin text NOT NULL CHECK (origin IN ('synthetic_demo', 'imported')),
    PRIMARY KEY (workspace_id, record_id)
);

CREATE TABLE review_rules (
    workspace_id uuid NOT NULL REFERENCES review_workspaces (workspace_id),
    rule_id text NOT NULL CHECK (octet_length(rule_id) BETWEEN 1 AND 256),
    original_wording text NOT NULL CHECK (octet_length(original_wording) BETWEEN 1 AND 256),
    kind text NOT NULL CHECK (kind = 'exclude_test_orders'),
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, rule_id)
);

CREATE TABLE review_queries (
    workspace_id uuid NOT NULL REFERENCES review_workspaces (workspace_id),
    query_id uuid NOT NULL,
    provenance jsonb NOT NULL CHECK (jsonb_typeof(provenance) = 'object'),
    result jsonb NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    rule_context jsonb NOT NULL CHECK (jsonb_typeof(rule_context) = 'array'),
    source_revision bigint NOT NULL CHECK (source_revision >= 0),
    source_records jsonb NOT NULL CHECK (jsonb_typeof(source_records) = 'array'),
    source_origins jsonb NOT NULL CHECK (jsonb_typeof(source_origins) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, query_id)
);

CREATE TABLE review_workbooks (
    workspace_id uuid NOT NULL REFERENCES review_workspaces (workspace_id),
    workbook_id uuid NOT NULL,
    title text NOT NULL CHECK (octet_length(title) BETWEEN 1 AND 256),
    latest_version bigint NOT NULL DEFAULT 0 CHECK (latest_version >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, workbook_id)
);

CREATE TABLE review_snapshots (
    workspace_id uuid NOT NULL,
    workbook_id uuid NOT NULL,
    snapshot_version bigint NOT NULL CHECK (snapshot_version > 0),
    -- Included in duplicate comparison, so changing a retry's original precondition
    -- cannot silently acknowledge another save.
    expected_version bigint NOT NULL CHECK (expected_version >= 0),
    export_id text NOT NULL CHECK (octet_length(export_id) BETWEEN 1 AND 256),
    workbook_session_id text NOT NULL CHECK (octet_length(workbook_session_id) BETWEEN 1 AND 256),
    exported_identity jsonb NOT NULL CHECK (jsonb_typeof(exported_identity) = 'object'),
    xlsx bytea NOT NULL CHECK (octet_length(xlsx) BETWEEN 1 AND 16777216),
    sha256 bytea NOT NULL CHECK (octet_length(sha256) = 32),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, workbook_id, snapshot_version),
    UNIQUE (workspace_id, workbook_id, workbook_session_id, export_id),
    FOREIGN KEY (workspace_id, workbook_id)
        REFERENCES review_workbooks (workspace_id, workbook_id)
);
