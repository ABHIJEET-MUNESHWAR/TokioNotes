-- TokioNotes — initial schema (idempotent, safe to re-run).
--
-- Mirrors PLAN.md §2:
--   * Users live on a single global shard (low write rate).
--   * `notes`, `note_acl` are hash-partitioned by owner_id.
--   * `note_snapshots` keeps the latest Y-CRDT state per note (one row,
--     upserted on every save) so bodies survive a gateway restart.
--   * `domain_events` is range-partitioned by `occurred_at` (monthly).
--   * `sagas` tracks long-running multi-step workflows (e.g. shareNote).
--
-- The Postgres-backed repository implementations are feature-gated behind
-- the `postgres` feature in `crates/infra`; the in-memory repos are still
-- the default for `cargo run -p tn-gateway` and `cargo test --workspace`.
--
-- Apply this file once per database. Idempotent thanks to IF NOT EXISTS /
-- ON CONFLICT clauses.

-- ---------- Extensions ------------------------------------------------------
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "citext";

-- ---------- Users -----------------------------------------------------------
CREATE TABLE IF NOT EXISTS users (
    id            UUID        PRIMARY KEY,
    email         CITEXT      UNIQUE NOT NULL,
    password_hash TEXT        NOT NULL,
    display_name  TEXT        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------- Notes (hash-partitioned by owner_id, 16 buckets) ----------------
CREATE TABLE IF NOT EXISTS notes (
    id         UUID        NOT NULL,
    owner_id   UUID        NOT NULL,
    title      TEXT        NOT NULL,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    version    BIGINT      NOT NULL DEFAULT 0,
    PRIMARY KEY (owner_id, id)
) PARTITION BY HASH (owner_id);

DO $$
DECLARE i INT;
BEGIN
    FOR i IN 0..15 LOOP
        EXECUTE format(
            'CREATE TABLE IF NOT EXISTS notes_p%1$s
             PARTITION OF notes
             FOR VALUES WITH (MODULUS 16, REMAINDER %2$s)',
            lpad(i::text, 2, '0'), i
        );
    END LOOP;
END $$;

CREATE INDEX IF NOT EXISTS idx_notes_owner_updated
    ON notes (owner_id, updated_at DESC);

-- ---------- Note ACLs -------------------------------------------------------
CREATE TABLE IF NOT EXISTS note_acl (
    note_id    UUID        NOT NULL,
    user_id    UUID        NOT NULL,
    role       SMALLINT    NOT NULL,         -- 0=Viewer 1=Editor 2=Owner
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (note_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_acl_user ON note_acl (user_id);

-- ---------- Note snapshots (Y-CRDT state) -----------------------------------
CREATE TABLE IF NOT EXISTS note_snapshots (
    note_id    UUID        NOT NULL,
    seq        BIGINT      NOT NULL,
    state      BYTEA       NOT NULL,         -- yrs encode_state_as_update_v1
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (note_id, seq)
);

-- ---------- Event store (range-partitioned by month) ------------------------
CREATE TABLE IF NOT EXISTS domain_events (
    id             UUID        NOT NULL,
    aggregate_id   UUID        NOT NULL,
    aggregate_type TEXT        NOT NULL,
    seq            BIGINT      NOT NULL,
    event_type     TEXT        NOT NULL,
    payload        JSONB       NOT NULL,
    metadata       JSONB       NOT NULL DEFAULT '{}'::jsonb,
    occurred_at    TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (aggregate_id, seq, occurred_at)
) PARTITION BY RANGE (occurred_at);

CREATE INDEX IF NOT EXISTS idx_events_type_time
    ON domain_events (event_type, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_aggregate
    ON domain_events (aggregate_id, seq);

-- Pre-create the current and next month's partitions so writes work out of
-- the box. In production a `pg_partman` cron / `pg_cron` job would extend
-- the rolling window automatically.
DO $$
DECLARE
    start_ts TIMESTAMPTZ := date_trunc('month', now());
    months   INT;
    pname    TEXT;
    p_from   TIMESTAMPTZ;
    p_to     TIMESTAMPTZ;
BEGIN
    FOR months IN 0..2 LOOP
        p_from := start_ts + make_interval(months => months);
        p_to   := p_from   + make_interval(months => 1);
        pname  := format('domain_events_%s', to_char(p_from, 'YYYY_MM'));
        EXECUTE format(
            'CREATE TABLE IF NOT EXISTS %I
             PARTITION OF domain_events
             FOR VALUES FROM (%L) TO (%L)',
            pname, p_from, p_to
        );
    END LOOP;
END $$;

-- ---------- Sagas -----------------------------------------------------------
CREATE TABLE IF NOT EXISTS sagas (
    id         UUID        PRIMARY KEY,
    saga_type  TEXT        NOT NULL,
    state      JSONB       NOT NULL,
    status     TEXT        NOT NULL,         -- Pending | Running | Completed | Compensating | Failed
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_sagas_status ON sagas (status, updated_at);

-- ---------- Migration tracking ---------------------------------------------
CREATE TABLE IF NOT EXISTS schema_migrations (
    version     TEXT        PRIMARY KEY,
    applied_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO schema_migrations (version)
VALUES ('0001_initial')
ON CONFLICT (version) DO NOTHING;

