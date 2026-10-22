-- Durable, bounded conversation checkpoints. The server selects workspace ownership.
CREATE TABLE review_agent_runs (
    workspace_id uuid NOT NULL REFERENCES review_workspaces(workspace_id),
    run_id uuid NOT NULL,
    state jsonb NOT NULL CHECK (octet_length(state::text) <= 2097152),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, run_id)
);
