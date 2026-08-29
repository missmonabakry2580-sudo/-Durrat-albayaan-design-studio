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

-- priority/deadline/project/next_action/approval_required/dependencies:
-- Mona's explicit task shape (she gave the exact JSON in her rearchitecture
-- request). All nullable/optional — a task from a plain "دوّني كذا" still
-- works with none of them set. New columns on an existing database are
-- added by db::migrate's ALTER TABLE statements, not by this CREATE TABLE
-- (which only ever runs for a brand-new database) — see db.rs.
CREATE TABLE IF NOT EXISTS tasks (
    id                TEXT PRIMARY KEY,
    title             TEXT NOT NULL,
    status            TEXT NOT NULL DEFAULT 'open', -- open | in_progress | done | cancelled
    source            TEXT,                          -- voice_note | quick_capture | email | manual
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    metadata          TEXT,
    priority          TEXT,    -- low | medium | high, Claude's judgment — not enforced here
    deadline          TEXT,    -- RFC3339, if Mona gave or implied one
    project           TEXT,    -- free-form grouping label, e.g. "تسجيل أحمد"
    next_action       TEXT,    -- the concrete next step, not just the task title
    approval_required INTEGER, -- 0/1 — whether finishing this task itself needs Mona's approval
    dependencies      TEXT     -- JSON array of task ids this one is blocked on
);

-- Amin's long-term conversation memory — persists across app restarts so
-- context carries over day to day instead of resetting every launch (the
-- in-memory `agent::Conversation` cap in agent.rs still bounds what's sent
-- to the API per turn; commands.rs keeps this table trimmed to a rolling
-- window too, so it grows, not without limit). Cleared only when Mona
-- explicitly starts a "New conversation".
CREATE TABLE IF NOT EXISTS conversation_history (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    ts      TEXT NOT NULL,
    role    TEXT NOT NULL,
    content TEXT NOT NULL -- JSON, mirrors agent::ChatMessage.content
);

-- Structured long-term memory (categorized facts Amin can recall, update,
-- and forget) — distinct from `conversation_history` above, which is raw
-- transcript replay, not memory Claude reasons over as facts. See
-- src-tauri/src/memory.rs. `(category, key)` isn't declared UNIQUE here
-- deliberately — memory.rs's remember() does its own look-up-then-
-- insert-or-update, since SQLite's ON CONFLICT needs a matching unique
-- index and a composite one adds a migration step for a constraint the
-- application layer already enforces.
CREATE TABLE IF NOT EXISTS memory_facts (
    id         TEXT PRIMARY KEY,
    category   TEXT NOT NULL,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS follow_ups (
    id               TEXT PRIMARY KEY,
    task_id          TEXT NOT NULL REFERENCES tasks(id),
    due_at           TEXT NOT NULL,
    escalation_stage TEXT NOT NULL DEFAULT 'friendly', -- friendly | firm | escalate_to_user
    status           TEXT NOT NULL DEFAULT 'pending',   -- pending | sent | resolved | cancelled
    created_at       TEXT NOT NULL
);
