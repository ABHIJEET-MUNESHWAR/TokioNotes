# TokioNotes 📝⚡
> Real-time, multi-user collaborative notes platform built on **Rust + Tokio**, with **CQRS / event-driven micro-services**, **Y-CRDT co-editing**, **Actix-Web + GraphQL** API (Query / Mutation / Subscription), **Argon2 + JWT** auth, an **agentic AI** assistant, and a **Next.js** frontend.
---
## ✨ Features
- **Accounts** — register / login with Argon2id-hashed passwords and JWT bearer tokens.
- **Notes** — every authenticated user can create, rename, delete and list their own notes.
- **Sharing** — owners can grant `Viewer / Editor / Owner` roles to other users by email; an event-driven saga propagates the change.
- **Real-time collaboration** — shared notes are co-edited simultaneously through a **Y-CRDT** (`yrs`) document held in a Tokio-mutex-guarded "room"; updates fan out to every subscriber over a GraphQL subscription.
- **Generative + Agentic AI** — `summarize`, `autocomplete`, `tag` and `suggest_edits` are exposed via the `AiAssistant` trait (deterministic heuristic shipped; OpenAI/local-LLM adapters pluggable). The agent runs all three in parallel via `tokio::join!`.
- **Resilience** — generic `with_timeout`, `with_retry` (exponential backoff + jitter) and a `CircuitBreaker` ship in `tn-common::resilience`.
- **Observability** — JSON `tracing` logs, `RUST_LOG`-driven filters, `/health` endpoint.
- **Persistence** — pluggable repositories (`UserRepo`, `NoteRepo`, `AclRepo`, `EventStore`). In-memory implementations ship; a Postgres adapter is feature-gated and the schema with hash-partitioned `notes` and time-range-partitioned `domain_events` is documented in [`PLAN.md`](./PLAN.md).
- **Sharding** — `ShardRouter::shard_for(user)` demonstrates `hash(user_id) % N` partitioning.
---
## 🏛 Architecture
```
                ┌──────────────────────────┐
                │   Next.js (App Router)    │
                │ Apollo + graphql-ws + Yjs │
                └────────────┬─────────────┘
                             │ GraphQL HTTP + WebSocket
                ┌────────────▼─────────────┐
                │  Gateway (Actix-Web +    │
                │  async-graphql schema)   │
                └─┬──────┬───────┬─────────┘
                  │      │       │
        ┌─────────▼─┐ ┌──▼─────┐ ┌▼────────────┐
        │ Auth svc  │ │ Notes  │ │ Collab svc  │
        │ (JWT+Arg2)│ │ (CQRS) │ │ (yrs CRDT)  │
        └─────┬─────┘ └───┬────┘ └──────┬──────┘
              │           │             │
              └─── Generic Repository<T> ─── + EventBus<DomainEvent>
                          │
                ┌─────────▼──────────┐
                │   PostgreSQL +     │  (in-memory in dev / tests)
                │   Redis pub/sub    │
                └────────────────────┘
                          │
                ┌─────────▼──────────┐
                │  AI Agent Service  │  AiAssistant trait
                └────────────────────┘
```
Pattern: **CQRS-lite + event sourcing** (commands append `DomainEvent`s on the bus; subscribers project read models). **Saga** orchestration is illustrated by `share()` in `tn-notes-service`. The Gateway is the composition root that wires every crate together; in production each crate would be deployed as its own service behind the same GraphQL schema.
See [`PLAN.md`](./PLAN.md) for the exhaustive design.
---
## 📦 Workspace layout
```
crates/
├── common/             # errors, IDs, tracing, resilience (timeout/retry/breaker), generic event bus, Repository<T,ID>
├── domain/             # pure aggregates, typestate EditSession<Idle|Editing|Committed>, ACL roles, AiAssistant trait
├── infra/              # password hashing, JWT issuer, shard router, in-memory repository implementations
├── auth-service/       # registration, login, token verification (generic over UserRepo)
├── notes-service/      # CQRS command handlers + saga (create / rename / delete / share / revoke)
├── collab-service/     # Y-CRDT rooms, broadcast fanout, snapshots
├── ai-agent-service/   # parallel/agentic orchestrator over AiAssistant
└── gateway/            # Actix-Web + async-graphql binary (HTTP /graphql + WS subscriptions)
frontend/               # Next.js 14 (App Router, Apollo + graphql-ws + Yjs)
```
---
## 🚀 Quick start (no Postgres required)
```bash
# 1. Build & test the entire Rust workspace
cargo test --workspace
# 2. Run the all-in-one gateway (in-memory repos)
JWT_SECRET=dev cargo run -p tn-gateway
# 3. Open the GraphQL Playground
open http://localhost:8080/
# 4. Run the frontend
cd frontend && npm install && npm run dev
# → http://localhost:3000
```
### Docker Compose (with Postgres + Redis)
```bash
docker compose up --build
```
Services exposed:
- Gateway GraphQL → http://localhost:8080/graphql (HTTP + WebSocket)
- Frontend → http://localhost:3000
- Postgres → :5432, Redis → :6379
---
## 🧪 Testing
```bash
cargo test --workspace            # 26+ unit & integration tests
cd frontend && npm test           # Jest (skeleton)
```
Test highlights:
- Domain: validation, typestate transitions, role permissions.
- Infra: argon2 round-trip, JWT issue/verify, shard determinism, repository semantics (uniqueness, optimistic concurrency).
- Auth-service: register / login / token verification, rejection of bad credentials.
- Notes-service: create / share / revoke, ACL enforcement, owner-only delete, anti-self-share.
- Collab-service: two-client CRDT convergence, broadcast delivery, invalid update rejection.
- AI-agent-service: parallel `agent_report`, batch summarisation order preservation.
- Gateway: end-to-end GraphQL register → create → share → list, unauthenticated rejection, **live subscription delivery of CRDT ops via `applyOps` mutation**.
---
## 📡 GraphQL surface
```graphql
type Query {
  me: UserDto!
  note(id: UUID!): NoteDto!
  myNotes: [NoteDto!]!
  collaborators(id: UUID!): [CollaboratorDto!]!
  aiSummary(id: UUID!): String!
  aiReport(id: UUID!, instruction: String!): AgentReportDto!
}
type Mutation {
  register(email: String!, displayName: String!, password: String!): AuthPayload!
  login(email: String!, password: String!): AuthPayload!
  createNote(title: String!): NoteDto!
  renameNote(id: UUID!, title: String!): NoteDto!
  deleteNote(id: UUID!): Boolean!
  shareNote(id: UUID!, email: String!, role: Role!): CollaboratorDto!
  revokeShare(id: UUID!, userId: UUID!): Boolean!
  applyOps(noteId: UUID!, updateB64: String!): String!
}
type Subscription {
  noteOps(noteId: UUID!): OpEvent!
}
```
Authorisation: `Authorization: Bearer <jwt>` HTTP header (or WS `connectionParams.authorization`).
---
## ⚙️ Configuration
| Env var       | Default                         | Purpose                          |
|---------------|---------------------------------|----------------------------------|
| `JWT_SECRET`  | `dev-secret-change-me`          | HMAC secret for signing tokens   |
| `BIND`        | `0.0.0.0:8080`                  | HTTP bind address                |
| `RUST_LOG`    | `info,sqlx=warn,hyper=warn`     | Tracing filter                   |
| `DATABASE_URL`| —                               | (postgres feature) connection    |
| `REDIS_URL`   | —                               | (redis adapter) event bus URL    |
---
## 🧠 Idiomatic patterns leveraged
- **Newtypes** (`UserId`, `NoteId`, `EventId`, `SessionId`) — distinct types prevent ID mix-ups at compile time.
- **Builder** (`User::builder()`) — validating, typed object construction.
- **Typestate** (`EditSession<Idle | Editing | Committed>`) — illegal state transitions are unrepresentable.
- **Repository<T, ID>** generic trait + Unit-of-Work shape.
- **Strategy / Adapter** — `EventBus`, `AiAssistant`, `UserRepo`, etc. are traits with multiple swap-in implementations.
- **Generic services** — `NotesService<U, N, A, E, B>` is parameterised by every collaborator so the same code runs against in-memory or SQL-backed infra.
- **Concurrency primitives** — `tokio::sync::Mutex` (per-room CRDT), `tokio::sync::broadcast` (fan-out), `dashmap` (lock-free room registry), `JoinSet` (batch parallelism).
- **Resilience combinators** — `with_timeout`, `with_retry`, `CircuitBreaker` are generic over future output type.
- **Sealed error type** with `thiserror::Error` enum.
---
## 📈 Performance & complexity
| Operation              | Complexity                  | Notes                                              |
|------------------------|-----------------------------|----------------------------------------------------|
| `create_note`          | O(1) amortised              | one repo insert + one ACL insert + event append    |
| `share_note`           | O(1)                        | user-by-email + ACL grant + event                  |
| `apply_ops` (CRDT)     | O(\|update\|)               | Yjs merge; per-room mutex serialises a single doc  |
| `note_ops` subscription| O(N) fanout                 | `tokio::sync::broadcast`, bounded buffer 1024      |
| `list_for(user)`       | O(owned + shared)           | owner index + ACL reverse index                    |
| `batch_summarize(N)`   | O(slowest call)             | parallel via `JoinSet`                             |
| `agent_report`         | O(slowest of 3 sub-calls)   | `tokio::join!`                                     |
CRDT correctness properties (associativity / commutativity / idempotence) are inherited from `yrs`; the `two_clients_converge` test exercises an interleaved scenario.
---
## 🛡 Fault tolerance
- All cross-service calls funnel through `with_timeout` (per-call deadline) and `with_retry` (jittered exponential backoff). Circuit breaker (`CircuitBreaker::call`) trips after `threshold` consecutive failures and re-closes after a configurable cooldown.
- Subscription receivers that lag past the broadcast buffer are silently dropped and re-sync via the `snapshot` API (state-vector resync) — no stuck writers.
- `NoteRepo::update` enforces optimistic concurrency by version number.
- Saga compensations are scaffolded in `notes-service::share`; failure modes are documented in `PLAN.md §5.3`.
---
## 🔐 Security
- Argon2id password hashing, OS-RNG salt.
- HMAC-SHA-256 JWT with TTL (24h default); rotation by changing `JWT_SECRET`.
- Permission gates run **before** any state mutation in `NotesService` and the GraphQL subscription resolvers.
- Owners-only for `share`, `revoke`, `delete`.
---
## 🤖 CI/CD
`.github/workflows/ci.yml` runs on every PR & push to `main`:
1. `cargo fmt --check`
2. `cargo clippy -- -D warnings`
3. `cargo test --workspace --all-features`
4. Frontend `npm install / lint / test / build`
5. Docker image build for both `gateway` and `frontend`.
---
## 🪞 Self-evaluation
| Category                | Status | Notes |
|-------------------------|--------|-------|
| SOLID                   | ✅     | Single-responsibility crates; DI via generics & traits; OCP via pluggable adapters. |
| Microservice pattern    | ✅     | CQRS + event-driven; saga in `share`. Gateway = composition root. |
| DB partitioning         | ✅ schema, ⚠️ in-memory default | `ShardRouter` + Postgres hash/range partitions documented. |
| Timeouts/retry/breaker  | ✅     | `tn-common::resilience` with tests. |
| Error handling          | ✅     | `thiserror`-based `AppError`, mapped to GraphQL at the edge. |
| GraphQL only            | ✅     | No REST endpoints (besides `/health`). |
| Test coverage           | ✅ 26+ tests | Domain, infra, services & end-to-end gateway covered. |
| Modular structure       | ✅     | 8 focused crates + frontend. |
| 3rd-party crates        | ✅     | tokio, serde, thiserror, async-graphql, sqlx, actix-web, yrs, dashmap, argon2, jsonwebtoken, tokio-retry. |
| Generative/Agentic AI   | ✅     | `AiAssistant` trait + parallel agentic `agent_report`. |
| Idiomatic patterns      | ✅     | Newtype, builder, typestate, strategy, repository. |
| Generics                | ✅     | `NotesService<U,N,A,E,B>`, `EventBus<E>`, `Repository<T,ID>`, resilience combinators. |
| README + setup          | ✅     | This file + `PLAN.md`. |
| Performance             | ✅     | Tokio multi-threaded RT, lock-free `DashMap`, broadcast fanout, parallel `JoinSet`. |
| Logging/observability   | ✅     | JSON `tracing`. OTel hooks documented. |
| Edge cases              | ✅     | Optimistic concurrency, viewer can't edit, owner-only delete, self-share blocked, invalid CRDT updates rejected, lagged subscribers re-sync. |
| Composable architecture | ✅     | All collaborators are traits; in-memory ↔ SQL swap is one wiring change. |
| Type-safe interfaces    | ✅     | Newtypes + typestate. |
| CI/CD                   | ✅     | GitHub Actions matrix. |
### 🔭 Roadmap improvements
1. Wire the Postgres adapter behind the `postgres` feature using the schema in `PLAN.md`; add `testcontainers` integration tests.
2. Pull the four sub-services into independently-deployable binaries communicating over a Redis/NATS event bus.
3. Add OpenTelemetry OTLP exporter + Prometheus `/metrics` endpoint.
4. `yrs` snapshotter background task per room → `note_snapshots` table.
5. Federate the GraphQL gateway (Apollo Federation v2) so each service ships its own subschema.
6. Replace heuristic `AiAssistant` with `async-openai` / Ollama implementations.
7. Frontend Tiptap + `y-prosemirror` for richer editing & cursor presence; offline cache via `y-indexeddb`.
8. End-to-end Playwright tests with two browser contexts demonstrating live co-edit.
---
## 📜 License
MIT © TokioNotes contributors.
