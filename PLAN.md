# TokioNotes — Implementation Plan

> Real-time, multi-user collaborative notes platform built on Rust + Tokio with CQRS, event-driven micro-services, CRDT-based co-editing, GraphQL API, and a Next.js frontend.

---

## 0. High-Level Architecture

```
                ┌──────────────────────────────┐
                │   Next.js Frontend (Apollo)  │
                │  HTTP + WebSocket (graphql-ws)│
                └──────────────┬───────────────┘
                               │ GraphQL
                ┌──────────────▼───────────────┐
                │   API Gateway (Actix-Web)    │
                │  async-graphql schema-stitch │
                └─┬──────┬────────┬─────────┬──┘
                  │      │        │         │
        ┌─────────▼─┐ ┌──▼─────┐ ┌▼───────┐ ┌▼────────────┐
        │ auth-svc  │ │notes-  │ │notes-  │ │ collab-svc  │
        │ (cmd+qry) │ │ svc    │ │query   │ │ (WS + CRDT) │
        │           │ │(cmd)   │ │svc     │ │             │
        └─────┬─────┘ └───┬────┘ └───┬────┘ └──────┬──────┘
              │           │          │             │
              │     ┌─────▼──────────▼─────┐ ┌─────▼──────┐
              │     │  PostgreSQL (sqlx)   │ │ Redis Pub/ │
              │     │  sharded by user_id  │ │  Sub + KV  │
              │     │  partitioned events  │ │ (event bus)│
              │     └──────────────────────┘ └────────────┘
              │
        ┌─────▼──────┐
        │ ai-agent   │  (async-openai / local LLM)
        │ service    │
        └────────────┘
```

- **Pattern**: CQRS + Event Sourcing (lite). Command services append domain events; query services maintain read-models from the bus.
- **Sagas**: cross-service workflows (e.g. `ShareNote`) orchestrated via `saga-coordinator` module in `common`.
- **Realtime**: `collab-svc` holds Y-CRDT (`yrs`) docs in memory, syncs ops over WebSocket subscriptions, persists snapshots periodically to Postgres, and broadcasts via Redis pub/sub for horizontal scale.

---

## 1. Cargo Workspace Layout (delivered MVP)

```
TokioNotes/
├── Cargo.toml                  # [workspace]
├── rust-toolchain.toml
├── docker-compose.yml
├── Dockerfile                  # multi-stage gateway image
├── .github/workflows/ci.yml
├── crates/
│   ├── common/                 # errors, IDs, tracing, resilience, eventbus, repository
│   ├── domain/                 # User, Note, ACL, EditSession typestate, AiAssistant
│   ├── infra/                  # password, JWT, ShardRouter, in-memory repos
│   ├── auth-service/           # AuthService<UserRepo>
│   ├── notes-service/          # NotesService<U,N,A,E,B> (CQRS + saga)
│   ├── collab-service/         # Y-CRDT rooms (Tokio Mutex + broadcast)
│   ├── ai-agent-service/       # parallel/agentic orchestrator
│   └── gateway/                # Actix-Web + async-graphql composition root
└── frontend/                   # Next.js 14 (App Router)
```

---

## 2. Database Schema & Partitioning (target Postgres adapter)

```sql
-- Users (global, not sharded)
CREATE TABLE users (
  id UUID PRIMARY KEY,
  email CITEXT UNIQUE NOT NULL,
  password_hash TEXT NOT NULL,
  display_name TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Notes: hash-partitioned by owner_id (16 partitions notes_p00..p15)
CREATE TABLE notes (
  id UUID PRIMARY KEY,
  owner_id UUID NOT NULL,
  title TEXT NOT NULL,
  deleted_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  version BIGINT NOT NULL DEFAULT 0
) PARTITION BY HASH (owner_id);

CREATE TABLE note_acl (
  note_id UUID NOT NULL,
  user_id UUID NOT NULL,
  role SMALLINT NOT NULL,        -- 0=Viewer 1=Editor 2=Owner
  granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (note_id, user_id)
);
CREATE INDEX idx_acl_user ON note_acl(user_id);

CREATE TABLE note_snapshots (
  note_id UUID NOT NULL,
  seq BIGINT NOT NULL,
  state BYTEA NOT NULL,          -- yrs encode_state_as_update_v1
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (note_id, seq)
);

-- Event store: range-partitioned by month (pg_partman-managed)
CREATE TABLE domain_events (
  id UUID NOT NULL,
  aggregate_id UUID NOT NULL,
  aggregate_type TEXT NOT NULL,
  seq BIGINT NOT NULL,
  event_type TEXT NOT NULL,
  payload JSONB NOT NULL,
  metadata JSONB NOT NULL,
  occurred_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (aggregate_id, seq, occurred_at)
) PARTITION BY RANGE (occurred_at);

CREATE TABLE sagas (
  id UUID PRIMARY KEY,
  saga_type TEXT NOT NULL,
  state JSONB NOT NULL,
  status TEXT NOT NULL,          -- Pending | Running | Completed | Compensating | Failed
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

| Concern | Strategy |
|---|---|
| Notes / ACL / snapshots | Hash partition by `owner_id`; physical shard chosen by `ShardRouter::shard_for(user)` |
| Events | Range partition by month; archive cold partitions to S3 |
| Users | Single global shard (low write rate); cached |
| Cross-shard queries | Avoided; query-side maintains denormalized read model |

---

## 3. GraphQL Schema (delivered)

See `crates/gateway/src/schema.rs`. Core operations:

- **Query**: `me`, `note`, `myNotes`, `collaborators`, `aiSummary`, `aiReport`.
- **Mutation**: `register`, `login`, `createNote`, `renameNote`, `deleteNote`, `shareNote`, `revokeShare`, `applyOps`.
- **Subscription**: `noteOps(noteId)` streams base64 Y-CRDT updates.

---

## 4. CRDT Integration (`yrs`)

1. Each `Note` has at most one `Room` containing a `yrs::Doc` and a single `Text "body"`.
2. Clients exchange `Update` blobs (`encode_v1`); server applies under a per-room `tokio::sync::Mutex` then re-broadcasts on a `tokio::sync::broadcast` channel.
3. Snapshots (full state encoded as a single update) are returned via `applyOps` and fetched by late joiners; in production a snapshotter task persists them to `note_snapshots` every N ops or T seconds.
4. Horizontal scale: a Redis pub/sub channel `collab:{note_id}` relays updates between collab pods (planned).
5. Convergence is intrinsic (Y-CRDT). Conflicts are never rejected. Lagged subscribers re-sync via `room.snapshot()`.

---

## 5. Event Flows

### 5.1 Sign-up
```
Client ─register─▶ Gateway ─▶ auth-svc
  argon2.hash → users.create → JWT issue
```

### 5.2 Create Note
```
Client ─createNote─▶ notes-svc
  notes.insert + acl.grant(owner) + events.append(NoteCreated)
  publish(NoteCreated) → query/projection consumers
```

### 5.3 Share Note (Saga)
```
notes-svc.share(actor, note, email, role)
  Step1: users.by_email(email)            (timeout + retry)
  Step2: acls.grant(NoteAcl)
  Step3: events.append(NoteShared) + bus.publish
  On Step2 failure → compensate Step1 (no-op)
  On Step3 failure → log; do not roll back grant (idempotent reapply)
```

### 5.4 Real-time Co-edit
```
ClientA WS subscribe noteOps(id)         ─▶ collab-svc.room.subscribe
ClientA applyOps(update)                 ─▶ collab-svc.room.apply
   broadcast → ClientB on same pod (direct)
   redis publish "collab:{id}" → other pods (planned)
   periodic snapshot → notes-svc → event store
```

### 5.5 AI Summarise
```
Client ─aiSummary(noteId)─▶ ai-agent-svc
  loads body via collab-svc.body_text
  AiAssistant.summarize → streamed back (planned: aiStream subscription)
```

---

## 6. Frontend Structure

```
frontend/
├── package.json / next.config.js / tsconfig.json
├── Dockerfile
└── src/
    ├── app/
    │   ├── layout.tsx        # ApolloProvider
    │   ├── page.tsx          # auth + note list
    │   └── notes/[id]/page.tsx  # Yjs-backed editor
    └── lib/
        ├── apollo.ts         # split HTTP/WS link with Bearer header
        ├── queries.ts        # gql documents
        └── Providers.tsx
```

Editor binds a local `Y.Doc` to a `<textarea>`, pushes updates via `applyOps`, and applies remote updates from the `noteOps` subscription. Production should swap to Tiptap + `y-prosemirror` and an `IndexedDB` provider for offline.

---

## 7. Docker Compose

`docker-compose.yml` brings up:

- `postgres:16-alpine`
- `redis:7-alpine`
- `gateway` (multi-stage Rust build)
- `frontend` (Node 20 multi-stage)

Healthcheck on Postgres gates the gateway start.

---

## 8. GitHub Actions

`.github/workflows/ci.yml`:

1. **rust** job: rustfmt, clippy `-D warnings`, `cargo test --workspace --all-features`.
2. **frontend** job: `npm install / lint / test / build` against Node 20.
3. **docker** job (depends on the previous two): builds `gateway` and `frontend` images via Buildx.

Future: `docker.yml` (publish to GHCR on tags), `deploy.yml` (manual env-gated `kubectl/helm`).

---

## 9. Testing Strategy

| Layer | Tooling | Scope |
|---|---|---|
| Domain (pure) | `cargo test` + `proptest` | Aggregate invariants, role rules, typestate transitions |
| Repositories | `testcontainers` Postgres (planned) | Real SQL against ephemeral DB |
| Service layer | `mockall` + in-memory repos | Branch coverage of command handlers/sagas |
| GraphQL | `async-graphql` test schema | Query/Mutation/Subscription contract |
| WebSocket / CRDT | Tokio tests (multi-client) | Convergence + broadcast |
| Frontend unit | Jest + RTL + MSW | Component + hook coverage |
| Frontend E2E | Playwright (two contexts) | Live co-edit |
| Performance | `criterion` (planned) | Documented complexity in `docs/benchmarks.md` |

Coverage gate (planned): `cargo-llvm-cov --fail-under-lines 95`.

---

## 10. Step-by-Step Build Order (delivered MVP marked ✅)

| Phase | Work | Status |
|---|---|---|
| 0 | Cargo workspace, toolchain, telemetry, CI skeleton | ✅ |
| 1 | Domain & infra foundations (`Repository<T>`, `EventBus<E>`, ShardRouter, in-memory repos) | ✅ |
| 2 | Auth service (Argon2 + JWT), unit tests | ✅ |
| 3 | Notes command service + saga, unit tests | ✅ |
| 4 | Notes query projection (currently colocated; consumer scaffold for split) | ⏳ |
| 5 | Collab service: rooms, broadcast, snapshots, two-client convergence test | ✅ |
| 6 | AI agent service: `agent_report`, `batch_summarize` | ✅ |
| 7 | Gateway: Actix-Web + async-graphql HTTP+WS, end-to-end tests | ✅ |
| 8 | Frontend: Apollo + Yjs editor | ✅ skeleton |
| 9 | Observability (OTel/Prometheus) | ⏳ tracing JSON ready |
| 10 | Postgres adapter, Redis bus, Helm/CD | ⏳ schema documented |

---

## 11. Idiomatic Patterns Used

| Pattern | Location |
|---|---|
| Newtype IDs | `tn-common::ids` |
| Builder | `User::builder()` |
| Typestate | `EditSession<Idle | Editing | Committed>` |
| Strategy / Adapter | `EventBus`, `AiAssistant`, `*Repo` |
| Generics | `NotesService<U,N,A,E,B>`, `Repository<T,ID>` |
| Resilience combinators | `with_timeout`, `with_retry`, `CircuitBreaker` |
| RAII / lifetime-scoped tx | `Room::apply` (decode within mutex scope) |

---

## 12. Self-Evaluation Snapshot

Tests passing: **all** crates green via `cargo test --workspace`.

Gaps to close:
- Wire Postgres adapter behind the `postgres` feature.
- Add OTel + Prometheus.
- Run `cargo-llvm-cov` to confirm coverage threshold.
- Split services into independent binaries; introduce Redis-backed `EventBus` adapter.
- Replace the heuristic `AiAssistant` with `async-openai` and stream tokens via a `Subscription`.

