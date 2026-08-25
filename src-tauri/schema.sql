-- Amin local database schema (Phase 0).
-- Everything here lives on-device only. No sync, no remote copy, no banking
-- data ever. Applied idempotently on every app start (see src/db.rs).

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Append-only: rows are inserted by audit::record() and never updated or
-- deleted by the app. This is the full "what did Amin do, at what risk
-- tier, and what was decided" trail behind the Audit Log / Confidence &
-- Evidence features.
CREATE TABLE IF NOT EXISTS audit_log (
    id        TEXT PRIMARY KEY,
    ts        TEXT NOT NULL,
    actor     TEXT NOT NULL,   -- 'amin' | 'user'
    action    TEXT NOT NULL,   -- short verb, e.g. "draft_email", "set_autonomy_level"
    risk_tier TEXT NOT NULL,   -- 'auto' | 'trusted_delegation' | 'confirm_high_risk' | 'excluded'
    decision  TEXT NOT NULL,   -- 'executed' | 'confirmed' | 'declined' | 'blocked'
    details   TEXT,            -- free-form JSON
    evidence  TEXT             -- links/citations backing the decision, if any
);

CREATE TABLE IF NOT EXISTS tasks (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'open', -- open | in_progress | done | cancelled
    source     TEXT,                          -- voice_note | quick_capture | email | manual
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    metadata   TEXT
);

CREATE TABLE IF NOT EXISTS follow_ups (
    id               TEXT PRIMARY KEY,
    task_id          TEXT NOT NULL REFERENCES tasks(id),
    due_at           TEXT NOT NULL,
    escalation_stage TEXT NOT NULL DEFAULT 'friendly', -- friendly | firm | escalate_to_user
    status           TEXT NOT NULL DEFAULT 'pending',   -- pending | sent | resolved | cancelled
    created_at       TEXT NOT NULL
);
