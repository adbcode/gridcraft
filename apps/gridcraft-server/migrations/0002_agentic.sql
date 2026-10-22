ALTER TABLE review_workbooks
    ADD COLUMN origin text NOT NULL DEFAULT 'user'
    CHECK (origin IN ('user', 'mock'));

CREATE TABLE agent_runs (
    workspace_id uuid NOT NULL REFERENCES review_workspaces (workspace_id),
    run_id uuid NOT NULL,
    start_request_id text NOT NULL CHECK (octet_length(start_request_id) BETWEEN 1 AND 128),
    start_fingerprint bytea NOT NULL CHECK (octet_length(start_fingerprint) = 32),
    workbook_session_id text NOT NULL CHECK (octet_length(workbook_session_id) BETWEEN 1 AND 256),
    status text NOT NULL CHECK (status IN ('running', 'awaiting_tool', 'completed', 'failed', 'interrupted')),
    messages jsonb NOT NULL CHECK (jsonb_typeof(messages) = 'array'),
    pending_tool jsonb NULL CHECK (pending_tool IS NULL OR jsonb_typeof(pending_tool) = 'object'),
    attachments jsonb NOT NULL CHECK (jsonb_typeof(attachments) = 'array'),
    action_count integer NOT NULL DEFAULT 0 CHECK (action_count BETWEEN 0 AND 1000),
    model_calls integer NOT NULL DEFAULT 0 CHECK (model_calls BETWEEN 0 AND 8),
    final_text text NULL CHECK (final_text IS NULL OR octet_length(final_text) <= 16384),
    error text NULL CHECK (error IS NULL OR octet_length(error) <= 512),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, run_id),
    UNIQUE (workspace_id, start_request_id)
);

CREATE TABLE agent_tool_receipts (
    workspace_id uuid NOT NULL REFERENCES review_workspaces (workspace_id),
    run_id uuid NOT NULL,
    tool_call_id text NOT NULL CHECK (octet_length(tool_call_id) BETWEEN 1 AND 128),
    tool_name text NOT NULL CHECK (octet_length(tool_name) BETWEEN 1 AND 128),
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    result jsonb NOT NULL,
    completed boolean NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, run_id, tool_call_id),
    FOREIGN KEY (workspace_id, run_id) REFERENCES agent_runs (workspace_id, run_id)
);
