//! Postgres-backed repository implementations.
//!
//! Gated behind the `postgres` cargo feature so the workspace stays
//! buildable without a database. Schema lives in `migrations/0001_initial.sql`:
//! - `users`                 — global, single shard
//! - `notes`                 — hash-partitioned by owner_id (16 buckets)
//! - `note_acl`              — collaborator role per (note, user)
//! - `domain_events`         — append-only, range-partitioned by month
//!
//! All adapters return `AppError` so the rest of the stack stays
//! storage-agnostic.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::Row;
use tn_common::error::{AppError, AppResult};
use tn_common::ids::{EventId, NoteId, UserId};
use tn_domain::events::DomainEvent;
use tn_domain::note::{Note, NoteAcl, Role};
use tn_domain::user::User;

use crate::repos::{AclRepo, EventStore, NoteRepo, SnapshotStore, UserRepo};
fn map_sqlx(e: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(db) = &e {
        if db.code().as_deref() == Some("23505") {
            return AppError::Conflict(db.message().to_string());
        }
    }
    AppError::Internal(e.to_string())
}

/// Build a lazy connection pool. `connect_lazy` does not block startup —
/// the first query opens the actual TCP connection. This keeps
/// `AppState::bootstrap` synchronous.
pub fn pool_from_url(url: &str) -> AppResult<PgPool> {
    PgPoolOptions::new()
        .max_connections(16)
        .connect_lazy(url)
        .map_err(|e| AppError::Internal(format!("pg pool: {e}")))
}

// ---------------------------------------------------------------- users -----

#[derive(Clone)]
pub struct PgUserRepo {
    pool: PgPool,
}

impl PgUserRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserRepo for PgUserRepo {
    async fn create(&self, u: &User) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, display_name, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(u.id.into_uuid())
        .bind(&u.email)
        .bind(&u.password_hash)
        .bind(&u.display_name)
        .bind(u.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| match map_sqlx(e) {
            AppError::Conflict(_) => AppError::Conflict("email already registered".into()),
            other => other,
        })?;
        Ok(())
    }

    async fn by_email(&self, email: &str) -> AppResult<Option<User>> {
        let row = sqlx::query(
            "SELECT id, email::text AS email, password_hash, display_name, created_at
             FROM users WHERE email = $1",
        )
        .bind(email.to_lowercase())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(row.map(row_to_user))
    }

    async fn by_id(&self, id: UserId) -> AppResult<Option<User>> {
        let row = sqlx::query(
            "SELECT id, email::text AS email, password_hash, display_name, created_at
             FROM users WHERE id = $1",
        )
        .bind(id.into_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(row.map(row_to_user))
    }
}

fn row_to_user(row: PgRow) -> User {
    User {
        id: UserId::from_uuid(row.get::<uuid::Uuid, _>("id")),
        email: row.get::<String, _>("email"),
        password_hash: row.get::<String, _>("password_hash"),
        display_name: row.get::<String, _>("display_name"),
        created_at: row.get::<DateTime<Utc>, _>("created_at"),
    }
}

// ---------------------------------------------------------------- notes -----

#[derive(Clone)]
pub struct PgNoteRepo {
    pool: PgPool,
}

impl PgNoteRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl NoteRepo for PgNoteRepo {
    async fn insert(&self, n: &Note) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO notes (id, owner_id, title, created_at, updated_at, version, deleted_at)
             VALUES ($1, $2, $3, $4, $5, $6, NULL)",
        )
        .bind(n.id.into_uuid())
        .bind(n.owner_id.into_uuid())
        .bind(&n.title)
        .bind(n.created_at)
        .bind(n.updated_at)
        .bind(n.version)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn update(&self, n: &Note) -> AppResult<()> {
        // Optimistic concurrency: refuse if a newer version already lives in
        // the row. The composite PK is (owner_id, id); supplying owner_id
        // also lets Postgres prune to a single hash partition.
        let res = sqlx::query(
            "UPDATE notes
                SET title       = $1,
                    updated_at  = $2,
                    version     = $3,
                    deleted_at  = CASE WHEN $4 THEN COALESCE(deleted_at, now()) ELSE NULL END
              WHERE owner_id    = $5
                AND id          = $6
                AND version     <= $3",
        )
        .bind(&n.title)
        .bind(n.updated_at)
        .bind(n.version)
        .bind(n.deleted)
        .bind(n.owner_id.into_uuid())
        .bind(n.id.into_uuid())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(AppError::Conflict("stale write".into()));
        }
        Ok(())
    }

    async fn by_id(&self, id: NoteId) -> AppResult<Option<Note>> {
        // Hash-partitioned by owner_id; without it Postgres scans all
        // 16 partitions. Acceptable for the (low-volume) single-note read
        // path — the hot myNotes path uses `for_user` which does prune.
        let row = sqlx::query(
            "SELECT id, owner_id, title, created_at, updated_at, version, deleted_at
             FROM notes WHERE id = $1",
        )
        .bind(id.into_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(row.map(row_to_note))
    }

    async fn for_user(&self, user: UserId) -> AppResult<Vec<Note>> {
        let rows = sqlx::query(
            "SELECT id, owner_id, title, created_at, updated_at, version, deleted_at
             FROM notes WHERE owner_id = $1 AND deleted_at IS NULL
             ORDER BY updated_at DESC",
        )
        .bind(user.into_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(row_to_note).collect())
    }

    async fn delete(&self, id: NoteId) -> AppResult<()> {
        let res =
            sqlx::query("UPDATE notes SET deleted_at = now(), updated_at = now() WHERE id = $1")
                .bind(id.into_uuid())
                .execute(&self.pool)
                .await
                .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(AppError::NotFound(id.to_string()));
        }
        Ok(())
    }
}

fn row_to_note(row: PgRow) -> Note {
    let deleted_at: Option<DateTime<Utc>> = row.get("deleted_at");
    Note {
        id: NoteId::from_uuid(row.get::<uuid::Uuid, _>("id")),
        owner_id: UserId::from_uuid(row.get::<uuid::Uuid, _>("owner_id")),
        title: row.get::<String, _>("title"),
        created_at: row.get::<DateTime<Utc>, _>("created_at"),
        updated_at: row.get::<DateTime<Utc>, _>("updated_at"),
        version: row.get::<i64, _>("version"),
        deleted: deleted_at.is_some(),
    }
}

// ----------------------------------------------------------------- acls -----

#[derive(Clone)]
pub struct PgAclRepo {
    pool: PgPool,
}

impl PgAclRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn role_to_i16(r: Role) -> i16 {
    match r {
        Role::Viewer => 0,
        Role::Editor => 1,
        Role::Owner => 2,
    }
}
fn i16_to_role(v: i16) -> Role {
    match v {
        2 => Role::Owner,
        1 => Role::Editor,
        _ => Role::Viewer,
    }
}

#[async_trait]
impl AclRepo for PgAclRepo {
    async fn grant(&self, acl: &NoteAcl) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO note_acl (note_id, user_id, role, granted_at)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (note_id, user_id) DO UPDATE
                SET role = EXCLUDED.role, granted_at = EXCLUDED.granted_at",
        )
        .bind(acl.note_id.into_uuid())
        .bind(acl.user_id.into_uuid())
        .bind(role_to_i16(acl.role))
        .bind(acl.granted_at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn revoke(&self, note: NoteId, user: UserId) -> AppResult<()> {
        sqlx::query("DELETE FROM note_acl WHERE note_id = $1 AND user_id = $2")
            .bind(note.into_uuid())
            .bind(user.into_uuid())
            .execute(&self.pool)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    async fn role_of(&self, note: NoteId, user: UserId) -> AppResult<Option<Role>> {
        let row = sqlx::query("SELECT role FROM note_acl WHERE note_id = $1 AND user_id = $2")
            .bind(note.into_uuid())
            .bind(user.into_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx)?;
        Ok(row.map(|r| i16_to_role(r.get::<i16, _>("role"))))
    }

    async fn collaborators(&self, note: NoteId) -> AppResult<Vec<NoteAcl>> {
        let rows = sqlx::query(
            "SELECT note_id, user_id, role, granted_at
             FROM note_acl WHERE note_id = $1 ORDER BY granted_at ASC",
        )
        .bind(note.into_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(rows
            .into_iter()
            .map(|r| NoteAcl {
                note_id: NoteId::from_uuid(r.get::<uuid::Uuid, _>("note_id")),
                user_id: UserId::from_uuid(r.get::<uuid::Uuid, _>("user_id")),
                role: i16_to_role(r.get::<i16, _>("role")),
                granted_at: r.get::<DateTime<Utc>, _>("granted_at"),
            })
            .collect())
    }

    async fn notes_for_user(&self, user: UserId) -> AppResult<Vec<NoteId>> {
        let rows = sqlx::query("SELECT note_id FROM note_acl WHERE user_id = $1")
            .bind(user.into_uuid())
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx)?;
        Ok(rows
            .into_iter()
            .map(|r| NoteId::from_uuid(r.get::<uuid::Uuid, _>("note_id")))
            .collect())
    }
}

// ----------------------------------------------------------- event store ----

#[derive(Clone)]
pub struct PgEventStore {
    pool: PgPool,
}

impl PgEventStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Best-effort `(aggregate_id, aggregate_type)` extraction from a domain
/// event. Aggregate id is whatever entity the event is about: the user for
/// `UserRegistered`, the note for everything else.
fn aggregate_of(evt: &DomainEvent) -> (uuid::Uuid, &'static str, &'static str, DateTime<Utc>) {
    match evt {
        DomainEvent::UserRegistered { user_id, at, .. } => {
            (user_id.into_uuid(), "User", "UserRegistered", *at)
        }
        DomainEvent::NoteCreated { note_id, at, .. } => {
            (note_id.into_uuid(), "Note", "NoteCreated", *at)
        }
        DomainEvent::NoteRenamed { note_id, at, .. } => {
            (note_id.into_uuid(), "Note", "NoteRenamed", *at)
        }
        DomainEvent::NoteShared { note_id, at, .. } => {
            (note_id.into_uuid(), "Note", "NoteShared", *at)
        }
        DomainEvent::NoteShareRevoked { note_id, at, .. } => {
            (note_id.into_uuid(), "Note", "NoteShareRevoked", *at)
        }
        DomainEvent::NoteDeleted { note_id, at, .. } => {
            (note_id.into_uuid(), "Note", "NoteDeleted", *at)
        }
        DomainEvent::NoteOpsApplied { note_id, at, .. } => {
            (note_id.into_uuid(), "Note", "NoteOpsApplied", *at)
        }
    }
}

fn event_id(evt: &DomainEvent) -> EventId {
    evt.id()
}

#[async_trait]
impl EventStore for PgEventStore {
    async fn append(&self, event: DomainEvent) -> AppResult<()> {
        let (agg_id, agg_type, evt_type, at) = aggregate_of(&event);
        let payload = serde_json::to_value(&event)
            .map_err(|e| AppError::Internal(format!("serialize event: {e}")))?;
        // Per-aggregate monotonic sequence. Race-free under serializable
        // isolation; under read-committed two concurrent writers may pick
        // the same `seq` and one will hit a unique-violation on the PK
        // (aggregate_id, seq, occurred_at) — caller can retry. For this
        // single-writer dev gateway it's fine.
        sqlx::query(
            "INSERT INTO domain_events
                (id, aggregate_id, aggregate_type, seq, event_type, payload, occurred_at)
             SELECT $1, $2, $3, COALESCE(MAX(seq), 0) + 1, $4, $5, $6
             FROM domain_events WHERE aggregate_id = $2",
        )
        .bind(event_id(&event).into_uuid())
        .bind(agg_id)
        .bind(agg_type)
        .bind(evt_type)
        .bind(payload)
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn list(&self) -> AppResult<Vec<DomainEvent>> {
        let rows =
            sqlx::query("SELECT payload FROM domain_events ORDER BY occurred_at ASC, seq ASC")
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let v: serde_json::Value = r.get("payload");
            let evt: DomainEvent = serde_json::from_value(v)
                .map_err(|e| AppError::Internal(format!("decode event: {e}")))?;
            out.push(evt);
        }
        Ok(out)
    }
}

// -------------------------------------------------------- snapshot store ----
//
// Y-CRDT document state per note. We keep one row per note (`seq = 0`)
// and upsert on every save: the CRDT itself is the journal, and the
// `domain_events` table records the audit trail of `NoteOpsApplied`, so
// note_snapshots only needs the latest fully-merged state for fast
// reload after a gateway restart. Schema (`migrations/0001_initial.sql`)
// supports historical snapshots via the composite PK `(note_id, seq)`,
// which we can lean on later if we want point-in-time recovery.

#[derive(Clone)]
pub struct PgSnapshotStore {
    pool: PgPool,
}

impl PgSnapshotStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SnapshotStore for PgSnapshotStore {
    async fn latest(&self, note: NoteId) -> AppResult<Option<Vec<u8>>> {
        let row = sqlx::query(
            "SELECT state FROM note_snapshots
             WHERE note_id = $1
             ORDER BY seq DESC
             LIMIT 1",
        )
        .bind(note.into_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(row.map(|r| r.get::<Vec<u8>, _>("state")))
    }

    async fn save(&self, note: NoteId, state: &[u8]) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO note_snapshots (note_id, seq, state, created_at)
             VALUES ($1, 0, $2, now())
             ON CONFLICT (note_id, seq) DO UPDATE
                SET state      = EXCLUDED.state,
                    created_at = now()",
        )
        .bind(note.into_uuid())
        .bind(state)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }
}
