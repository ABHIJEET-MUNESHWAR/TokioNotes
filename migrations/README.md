# Database migrations

Plain-SQL, idempotent, versioned forward-only migrations for the TokioNotes
PostgreSQL schema documented in [`PLAN.md §2`](../PLAN.md). Each file is
named `NNNN_<slug>.sql` and is applied in lexicographic order.

| File | What it does |
|---|---|
| `0001_initial.sql` | Extensions (`uuid-ossp`, `citext`); `users`; hash-partitioned `notes` (16 shards `notes_p00 … notes_p15`); `note_acl`; `note_snapshots`; range-partitioned `domain_events` with the current + next two monthly partitions pre-created; `sagas`; a `schema_migrations` ledger table. |

## How to apply

### Option A — automatic (Docker Compose)

`docker compose up --build` now starts a one-shot **`migrator`** service after
Postgres becomes healthy:

```
postgres → healthy → migrator (psql -f /migrations/*.sql) → exits 0 → gateway starts
```

Nothing else to do; the schema is in place by the time the gateway boots.

### Option B — `psql` directly

```bash
PGPASSWORD=tn psql -h localhost -U tn -d tokionotes -f migrations/0001_initial.sql
```

Re-running is safe — every `CREATE` uses `IF NOT EXISTS`, partitions are
created in idempotent loops, and the `schema_migrations` insert is
`ON CONFLICT DO NOTHING`.

### Option C — `sqlx-cli` (recommended once the Postgres adapter lands)

```bash
cargo install sqlx-cli --no-default-features --features postgres
export DATABASE_URL=postgres://tn:tn@localhost:5432/tokionotes
sqlx migrate run                # applies any pending migration
sqlx migrate add my_change      # scaffolds a new file in this folder
```

The Rust `tn-infra` crate will, behind the planned `postgres` feature, embed
this folder via `sqlx::migrate!("./migrations")` so `cargo run -p tn-gateway`
applies migrations on startup.

## Conventions

- **Idempotent.** Every statement must be safe to re-run (`IF NOT EXISTS`,
  `ON CONFLICT`, `DO $$ … $$` blocks). This makes recovery and partial
  application trivial.
- **Forward-only.** No down-migrations; rollbacks are done with a new
  `NNNN_revert_xyz.sql` file. This matches the production discipline used
  by sqlx, golang-migrate, dbmate, etc.
- **One logical change per file.** Keep diffs reviewable.
- **Partitioning.** New partitions for `notes_p*` and `domain_events_*` should
  be added in dedicated migration files when the existing modulus or the
  rolling time window needs to grow.

