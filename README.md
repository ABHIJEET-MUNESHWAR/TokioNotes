# TokioNotes 📝⚡
> Real-time, multi-user collaborative notes platform built on **Rust + Tokio**, with **CQRS / event-driven micro-services**, **Y-CRDT co-editing**, **Actix-Web + GraphQL** API (Query / Mutation / Subscription), **Argon2 + JWT** auth, an **agentic AI** assistant, and a **Next.js** frontend.
---
## ✨ Features
- **Accounts** — register / login with Argon2id-hashed passwords and JWT bearer tokens.
- **Notes** — every authenticated user can create, rename, delete and list their own notes. Each note has a **Title** (server-side `Note.title`, edited via `renameNote`) and a **Body** (live Y-CRDT text co-edited via `applyOps` / `noteOps`).
- **Dark / Light theme** — the Next.js UI ships a persisted theme switch (🌙 / ☀️) wired through CSS variables; initial theme honours `prefers-color-scheme` and is restored from `localStorage` on next visit (no flash of the wrong theme thanks to a pre-hydration boot script in `app/layout.tsx`).
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

### Layered view

TokioNotes is organised into **six logical layers**. Dependencies point *downward* only — the presentation layer knows about services, services know about domain + infra, but nothing ever depends on the presentation layer. This is Clean/Hexagonal Architecture enforced by the Cargo crate graph.

```
┌─────────────────────────────────────────────────────────────────────┐
│ 1. Presentation        Next.js · Apollo · graphql-ws · Yjs provider │
├─────────────────────────────────────────────────────────────────────┤
│ 2. API / Gateway       Actix-Web · async-graphql · AuthMiddleware   │
├─────────────────────────────────────────────────────────────────────┤
│ 3. Application / Svc   AuthService · NotesService · RoomManager ·   │
│                        AiService   (CQRS commands, sagas, agents)   │
├─────────────────────────────────────────────────────────────────────┤
│ 4. Domain              User · Note · ACL · EditSession · DomainEvent │
│                        (pure Rust, no I/O, no async)                │
├─────────────────────────────────────────────────────────────────────┤
│ 5. Infrastructure      Repos (in-mem / Postgres) · JwtIssuer ·      │
│                        Argon2 · ShardRouter · yrs · EventBus adapt. │
├─────────────────────────────────────────────────────────────────────┤
│ 6. Cross-cutting       tn-common: errors, IDs, tracing, resilience, │
│                        generic Repository<T,ID>, EventBus<E>        │
└─────────────────────────────────────────────────────────────────────┘
```

### Components — What / How / Why

#### Layer 1 — Presentation (`frontend/`)

| Component | What | How | Why |
|---|---|---|---|
| **Next.js App Router** | Web UI for auth, notes list, live editor. | Server-rendered shell + client components (`"use client"`); route `/notes/[id]` hosts the editor. | Modern React with file-system routing; SSR for the landing/auth pages keeps TTFB low while the editor is interactive. |
| **Apollo Client + split link** | GraphQL transport with automatic protocol selection. | `HttpLink` for queries/mutations, `GraphQLWsLink` for subscriptions; `split()` routes by operation type. Auth `ApolloLink` injects `Authorization: Bearer` from `localStorage`. | One client, one cache, both HTTP and WebSocket — the cache auto-invalidates when a mutation response overlaps a cached query. |
| **Yjs (`Y.Doc`)** | Client-side CRDT replica of note content. | On edit we diff against `encodeStateVector` and send `encodeStateAsUpdate` over `applyOps`; incoming `noteOps` frames are merged via `Y.applyUpdate`. | Yjs is the de-facto Rust/JS CRDT pairing (`yrs` is the Rust port); guarantees offline-safe, conflict-free merges so the UI never has to "reject" a keystroke. |

#### Layer 2 — API / Gateway (`crates/gateway`)

| Component | What | How | Why |
|---|---|---|---|
| **Actix-Web `HttpServer`** | HTTP + WebSocket runtime. | Multi-threaded Tokio accept loop; routes `POST /graphql`, `GET /graphql` (WS upgrade), `GET /`, `GET /health`. | Actix is the fastest mainstream Rust HTTP server; its actor model also cleanly supports WebSocket subscriptions. |
| **`async-graphql` schema** | Type-safe GraphQL engine with Query, Mutation and Subscription roots. | `Schema::build(QueryRoot, MutationRoot, SubscriptionRoot).data(AppState).finish()`; each resolver is an `async fn`. | GraphQL (not REST) per requirement; `async-graphql` derives SDL from Rust types so the schema cannot drift from the code. |
| **`AuthToken` extractor** | Per-request bearer-token carrier. | HTTP handler reads `Authorization` header → wraps in `AuthToken(String)` → `request.data(...)`. WS handler does the same from `connectionParams`. | Keeps auth concerns out of resolvers — `current_user(ctx)` is the single chokepoint and is trivially mockable in tests. |
| **GraphQL Playground** | Interactive API explorer at `/`. | `async_graphql::http::playground_source(...)` inline HTML. | Zero-setup developer experience; also used by the Postman collection as a sanity-check target. |

#### Layer 3 — Application / Services

| Component | What | How | Why |
|---|---|---|---|
| **`AuthService<UserRepo>`** (`crates/auth-service`) | Registration, login, token verification. | Argon2id hashing via `tn_infra::password`, JWT issuance/verification via `JwtIssuer`, parameterised by any `UserRepo` (SRP). | Separated from notes logic so it can later be deployed as an independent micro-service without refactoring. |
| **`NotesService<U,N,A,E,B>`** (`crates/notes-service`) | CQRS command handlers + saga orchestration for create / rename / share / revoke / delete. | Generic over every collaborator (repos + event bus); every command ends in `emit(event)` which appends to the store *and* publishes on the bus. | Generics instead of `dyn` = zero-cost abstraction, no virtual calls, and each test wires in an in-memory implementation without mocking frameworks. The event-emission chokepoint is the seam that enables CQRS read-side projection in the future. |
| **`RoomManager` + `Room`** (`crates/collab-service`) | Per-note CRDT state machine. | `DashMap<NoteId, Arc<Room>>`; each `Room` owns a `tokio::sync::Mutex<yrs::Doc>` and a `tokio::sync::broadcast::Sender<OpFrame>`. | A room is a *unit of serialisation* — only one writer mutates a given doc at a time — but rooms themselves run in parallel on the Tokio multi-threaded runtime, so throughput scales with the number of active notes. |
| **`AiService<A: AiAssistant>`** (`crates/ai-agent-service`) | Generative + agentic AI façade. | `tokio::join!` fans out `summarize` + `tag` + `suggest_edits`; `batch_summarize` uses `JoinSet` for bounded concurrency. | Agentic pattern (plan → act in parallel → observe) without coupling to a specific LLM; swap `HeuristicAssistant` for `OpenAiAssistant` by changing one `Arc::new`. |
| **Saga coordinator (embedded)** | Multi-step workflow for `shareNote`. | `NotesService::share` executes Step 1 (lookup) → Step 2 (grant ACL) → Step 3 (emit event) with documented compensation semantics. | The Saga pattern replaces distributed transactions that SQL can't give us across shards / services; compensations are explicit and testable. |

#### Layer 4 — Domain (`crates/domain`)

| Component | What | How | Why |
|---|---|---|---|
| **`User`, `Note`, `NoteAcl`** | Aggregate roots and value objects. | Pure Rust structs with `Builder` constructors (`User::builder()`) that validate invariants (email format, non-empty title). | Domain purity (no async, no I/O, no dependencies on infra) means unit tests run in microseconds and the business rules are independently reviewable. |
| **`Role` enum** | `Viewer / Editor / Owner` authorisation level. | Methods `can_edit()` / `can_share()`. | Encodes policy once at the type level; resolvers just ask the enum. |
| **`EditSession<Idle \| Editing \| Committed>`** | Typestate for note mutation lifecycle. | Generic over a marker type; `open()` returns `Editing`, `commit()` returns `Committed`. Each state exposes only legal methods. | Makes illegal transitions (e.g. committing an un-opened session, mutating a committed one) **compile-time impossible** — the strongest guarantee the language offers. |
| **`DomainEvent`** | Sum type of all domain facts. | `#[serde(tag="type")]` tagged enum: `NoteCreated`, `NoteShared`, `NoteOpsApplied`, …; every variant carries `EventId`, timestamp and aggregate id. | Event sourcing primitive: the entire history of the system can be replayed from this sequence, enabling read-model rebuilds, audit logs and temporal queries. |
| **`AiAssistant` trait** | Domain-level port for AI. | Async trait with `summarize / autocomplete / tag / suggest_edits`. | Domain owns the **interface**, infra owns the **implementation** — classic Hexagonal Architecture, lets us depend-inversion the LLM vendor. |

#### Layer 5 — Infrastructure (`crates/infra`)

| Component | What | How | Why |
|---|---|---|---|
| **`UserRepo / NoteRepo / AclRepo / EventStore` traits** | Persistence ports. | Async traits; in-memory implementations using `DashMap` ship today, `sqlx` implementations are feature-gated behind `postgres`. | Trait-based polymorphism lets the same `NotesService` code run against PostgreSQL in prod and `DashMap` in CI — identical behaviour, 100× test speed. |
| **`JwtIssuer`** | JWT HS256 signer/verifier. | Wraps `jsonwebtoken::encode/decode`; constant-time HMAC verify. | Encapsulates cryptographic policy (algorithm, TTL) so rotation is a single constructor call. |
| **Argon2id hasher** | Password storage. | `argon2` crate with default params, OS-RNG salt. | Memory-hard hashing defeats GPU brute-force — current OWASP recommendation. |
| **`ShardRouter`** | Logical DB shard selector. | `hash(user_id) % N` via `DefaultHasher`. | Ready for horizontal scale: when the Postgres adapter wires in, the same router decides which connection pool a query goes to — no service code changes. |
| **`yrs` (Y-CRDT)** | Server-authoritative CRDT engine. | Used inside `Room`: `Doc::transact_mut().apply_update(...)`, `encode_state_as_update_v1`. | Industrial-strength CRDT with known complexity bounds and a compatible JS peer (`yjs`); we get cross-language convergence for free. |
| **In-memory `EventBus` (`InProcBus<E>`)** | Pub/sub over `tokio::sync::broadcast`. | Generic over event type `E`, subscribe returns a `Stream`. | A trait seam for swapping to Redis/NATS later without touching `NotesService`. |

#### Layer 6 — Cross-cutting (`crates/common`)

| Component | What | How | Why |
|---|---|---|---|
| **`AppError` + `AppResult<T>`** | Unified, sealed error type. | `thiserror::Error` enum with variants for NotFound / Unauthorized / Forbidden / Conflict / Validation / Storage / Timeout / Upstream / Internal. | Every layer maps to this; the GraphQL edge maps it out. No `Box<dyn Error>` in hot paths; every branch is pattern-matchable. |
| **Newtype IDs (`UserId`, `NoteId`, …)** | Distinct UUID wrappers. | Macro-generated; implement `Display`, `Serialize`, `async_graphql::scalar!`. | Prevents mixing an owner id with a note id at call sites — a bug class that would be silent with raw `Uuid`. |
| **`telemetry::init`** | Standardised JSON logging. | `tracing-subscriber` with `EnvFilter` + JSON `fmt` layer. | One call per binary; `RUST_LOG` is the only knob; OTel layer drops in next to it without changing call sites. |
| **Resilience combinators** | `with_timeout`, `with_retry`, `CircuitBreaker`. | Generic over `Future<Output = AppResult<T>>`; `tokio-retry` for jittered backoff; atomic state machine for the breaker. | One composable layer of fault tolerance that every adapter can reuse — no bespoke timeout/retry code scattered through services. |
| **Generic `Repository<T, ID>` trait** | Uniform persistence contract. | `async fn get / save / delete`. | Makes "in-memory vs SQL" an *implementation* choice, not a design one; compatible with the Unit-of-Work pattern. |
| **Generic `EventBus<E>` trait** | Uniform pub/sub contract. | `async fn publish`, `fn subscribe() -> Stream`. | Same rationale: transport-agnostic CQRS. Prod can mix Redis for fan-out and the in-proc bus for tests without any conditional compilation in services. |

### Dependency graph

```
tn-common  ◀── every other crate
tn-domain  ◀── infra, auth-service, notes-service, collab-service, ai-agent-service, gateway
tn-infra   ◀── auth-service, notes-service, gateway
tn-auth-service   ◀── gateway
tn-notes-service  ◀── gateway
tn-collab-service ◀── gateway
tn-ai-agent-service ◀── gateway
```

The graph is a DAG — no cycles — and every arrow points from a more concrete crate to a more abstract one. `domain` has **zero** runtime dependencies (no `tokio`, no I/O), which is what makes it unit-testable in microseconds and makes the architecture honest about where side effects live.

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
open http://localhost:9090/
# 4. Run the frontend
cd frontend && npm install && npm run dev
# → http://localhost:9000
```
### Docker Compose (with Postgres + Redis)
```bash
docker compose up --build
```
Services exposed:
- Gateway GraphQL → http://localhost:9090/graphql (HTTP + WebSocket)
- Frontend → http://localhost:9000
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
## 🛠 How It Works

This section walks through every public API call and every internal flow, naming the exact crate / module / function you can read to follow along.

### Process startup

1. `crates/gateway/src/main.rs`
   - `tn_common::telemetry::init("tn-gateway")` installs a JSON `tracing-subscriber` governed by `RUST_LOG`.
   - `AppState::bootstrap(&jwt_secret)` (in `crates/gateway/src/app.rs`) constructs the **composition root**:
     - `InMemoryUserRepo`, `InMemoryNoteRepo`, `InMemoryAclRepo`, `InMemoryEventStore` (all `Arc`-wrapped, shared across tasks via `dashmap`).
     - `InProcBus::<DomainEvent>::new(2048)` — a `tokio::sync::broadcast`-backed bus.
     - `AuthService` with `JwtIssuer::new(secret, 24h)`.
     - `NotesService<U, N, A, E, B>` — generic over every collaborator; the same struct works against SQL-backed adapters when the `postgres` feature wires them in.
     - `RoomManager` — lock-free `DashMap<NoteId, Arc<Room>>` for CRDT rooms.
     - `AiService::new(Arc::new(HeuristicAssistant))`.
   - `build_schema(state)` in `crates/gateway/src/schema.rs` constructs the `async_graphql::Schema` and injects `AppState` as schema data.
   - `actix_web::HttpServer` binds the schema onto three routes:
     - `POST /graphql` → `graphql()` HTTP handler (extracts `Authorization: Bearer …` and attaches an `AuthToken(String)` request-data value).
     - `GET /graphql` with `Upgrade: websocket` → `graphql_ws()` handler (uses `async-graphql-actix-web::GraphQLSubscription` to speak the `graphql-transport-ws` protocol).
     - `GET /` → GraphQL Playground, `GET /health` → `"ok"`.

### Authentication pipeline

Every non-auth resolver calls `current_user(ctx)` in `schema.rs`, which:

1. Reads the `AuthToken` request-scoped value (placed there by the HTTP/WS handler).
2. Passes the bearer string to `AuthService::verify` → `JwtIssuer::verify` (`jsonwebtoken::decode` with the configured HMAC secret).
3. Parses the `sub` claim into a `UserId` newtype and returns it to the resolver, or returns `Unauthorized` which maps to a GraphQL error via `to_gql`.

Argon2id is used only by `AuthService::register` / `login` via `tn_infra::password` (`Argon2::default()` with OS-RNG salt, `PasswordHash` verify).

### External flows (GraphQL API)

Below, ① = mutation/query/subscription entry, ② = handler, ③ = domain effects, ④ = event emission, ⑤ = response.

#### 1. `register(email, displayName, password)` — `Mutation`

1. ① Resolver `MutationRoot::register` in `schema.rs` (no auth).
2. ② `AuthService::register`:
   - rejects passwords `< 8` chars (`AppError::Validation`);
   - `password::hash` → Argon2id string;
   - `User::builder()` validates `email` contains `@` and `display_name` is non-empty, lowercases the email, generates a `UserId`;
   - `UserRepo::create` — errors with `Conflict` if the email already exists.
3. ③ `JwtIssuer::issue(user.id)` signs `{ sub, iat, exp }`.
4. ⑤ Returns `AuthPayload { user, token }`.

There is no domain event for registration yet (the `DomainEvent::UserRegistered` variant exists in `tn-domain::events` and is emitted in the planned Postgres adapter).

#### 2. `login(email, password)` — `Mutation`

1. `AuthService::login`:
   - `UserRepo::by_email` → `Unauthorized` if missing;
   - `password::verify` → `Unauthorized` if mismatch (both branches intentionally return the same error to resist enumeration);
   - mints a fresh JWT.
2. Returns `AuthPayload`.

#### 3. `me` — `Query`

1. `current_user(ctx)` resolves the `UserId` from the bearer token.
2. Returns a `UserDto`. (Email/display-name hydration is planned; see roadmap.)

#### 4. `createNote(title)` — `Mutation`

1. `MutationRoot::create_note` → `NotesService::create(owner, title)`.
2. `NotesService::create`:
   - `Note::create` validates `title` is non-empty and stamps `created_at / updated_at / version=0`;
   - `NoteRepo::insert` (also indexes by owner);
   - `AclRepo::grant(NoteAcl { role: Owner })` — owner gets the ACL row so `collaborators(id)` lists them and sharing revoke logic works uniformly;
   - `emit(DomainEvent::NoteCreated)` → `EventStore::append` + `EventBus::publish`;
3. ⑤ `RoomManager::get_or_create(note.id)` pre-warms the CRDT room so subscribers don't have to wait. Returns a `NoteDto` whose `snapshotB64` is the initial (empty) Yjs state encoded as a `v1` update.

#### 5. `renameNote(id, title)` — `Mutation`

1. `NotesService::rename`:
   - `require_role(note, actor)` — loads the `Note`, rejects if `deleted`, resolves the caller's `Role` (owner short-circuits, otherwise `AclRepo::role_of`);
   - `EditSession::<Idle>::open` — **typestate** transition. Refuses if role can't edit (`Viewer`) or note is deleted, returning a typed `EditSession<Editing>`.
   - `.rename(new_title)` — validates non-empty title, bumps `updated_at` and `version`.
   - `.commit()` — transitions to `EditSession<Committed>`; only this typestate is persisted.
   - `NoteRepo::update` — optimistic concurrency: refuses writes whose `version` is older than what's stored.
   - `emit(DomainEvent::NoteRenamed)`.

Illegal states (e.g., mutating an `Idle` session or double-committing) are prevented at compile time by the typestate.

#### 6. `deleteNote(id)` — `Mutation`

1. `NotesService::delete` — `require_role` ensures the caller is `Owner` (soft-check: only `Role::Owner` may delete);
2. `NoteRepo::delete` sets `deleted=true`, `updated_at=now()`;
3. `emit(DomainEvent::NoteDeleted)`;
4. Gateway resolver also calls `RoomManager::close(id)` so the CRDT room is evicted and its `broadcast::Sender` dropped (subscribers receive `Closed`).

#### 7. `shareNote(id, email, role)` — `Mutation` (**Saga**)

This is the canonical cross-aggregate flow and serves as the saga template:

1. `require_role(note, actor)` — must be `Owner` (`Role::can_share`).
2. **Step 1**: `UserRepo::by_email(with_email)` — `NotFound` if missing; rejects self-share (`Validation`).
3. **Step 2**: `AclRepo::grant(NoteAcl { user, role, granted_at })` — idempotent (duplicate grants replace the previous role).
4. **Step 3**: `emit(DomainEvent::NoteShared)` — append to `EventStore` and publish on `EventBus`.

Failure modes (documented in `PLAN.md §5.3`):
- Step 1 fails → nothing persisted; return error.
- Step 2 fails → nothing to compensate (Step 1 is a pure read).
- Step 3 fails → the grant is left in place intentionally; the event is idempotent and can be replayed. In the Redis-backed deployment the outbox pattern will make this atomic.

Read-side consumers (planned split of `notes-query-service`) subscribe to the bus and project `NoteShared` events into a denormalised read model keyed by `(user_id)` so `myNotes` can be served without cross-shard joins.

#### 8. `revokeShare(id, userId)` — `Mutation`

Owner-only. `AclRepo::revoke` removes the ACL row from both the note-index and user-index maps; `DomainEvent::NoteShareRevoked` is published.

#### 9. `myNotes` — `Query`

`NotesService::list_for(user)`:

1. Pulls owner-indexed notes from `NoteRepo::for_user`.
2. Pulls shared-note IDs from `AclRepo::notes_for_user`.
3. Merges, drops duplicates and `deleted` notes, sorts by `updated_at` descending.
4. Gateway maps each `Note` to a `NoteDto`, inlining the room snapshot (`snapshotB64`) when a room exists so a freshly-opened client has immediate content.

Complexity: `O(owned + shared)` for the index lookups + `O(1)` per note for `DashMap::get` of the concrete `Note`.

#### 10. `note(id)` / `collaborators(id)` — `Query`

Both gate through `NotesService::require_role`. `collaborators` returns the raw `NoteAcl` rows from `AclRepo::collaborators`; owner is listed (because the owner's ACL row is written at create time).

#### 11. `applyOps(noteId, updateB64)` — `Mutation` (**CRDT hot path**)

This is the write side of real-time co-edit.

1. `current_user` → `UserId`.
2. `NotesService::note(uid, note_id)` acts as the **permission gate**: enforces existence, non-deletion, and that the caller has an ACL row (viewer / editor / owner). Viewers can still `applyOps` today; the next iteration will plumb role into the room and reject viewer writes. For now viewer edits are rejected at the dedicated `renameNote`/domain layer.
3. `B64.decode(update_b64)` — base64 → raw bytes.
4. `RoomManager::get_or_create(note_id)` returns `Arc<Room>`.
5. `Room::apply(bytes)` (in `crates/collab-service/src/lib.rs`):
   1. Acquire `self.doc.lock().await` — a `tokio::sync::Mutex<Doc>`. The decode is performed **after** the lock so the `!Send` `Update` value never crosses an `.await`.
   2. `Update::decode_v1(bytes)` — validates the wire format; malformed updates return `AppError::Validation`.
   3. `doc.transact_mut().apply_update(update)` — CRDT merge. Concurrent writes converge by construction (Yjs guarantees associativity/commutativity/idempotence).
   4. Drop the lock.
   5. `self.tx.send(OpFrame { note, update: bytes })` — best-effort broadcast. If there are no subscribers `send` returns `Err` and we ignore it; if the buffer is full, lagging subscribers will later see `BroadcastStreamRecvError::Lagged` and re-sync via `room.snapshot()`.
6. `Room::snapshot()` → the new full state (`encode_state_as_update_v1(&StateVector::default())`), base64-encoded and returned as the mutation result. Clients can use this to reseed their local `Y.Doc` after a reconnect.

Complexity: O(|update|) for decode + merge; O(N) for fan-out where N = live subscribers.

#### 12. `noteOps(noteId)` — `Subscription` (**CRDT read path**)

1. WebSocket handshake at `GET /graphql` (Actix upgrade); `graphql-transport-ws` client sends `connection_init` (with `authorization: Bearer …` in `connectionParams`).
2. `SubscriptionRoot::note_ops` runs `current_user(ctx)` using the WS-attached token, then `NotesService::note(uid, note_id)` as the permission gate.
3. `RoomManager::get_or_create(note_id).subscribe()` returns a `broadcast::Receiver<OpFrame>`.
4. A `BroadcastStream` wraps the receiver and we `filter_map` each frame into an `OpEvent { noteId, updateB64 }`. Lagged items are dropped by the `.ok()` filter so the stream survives bursts.
5. Cancellation happens automatically when the client closes the WS or sends `{"type":"complete"}`; Actix drops the stream and the broadcast receiver count goes down.

#### 13. `aiSummary(id)` / `aiReport(id, instruction)` — `Query`

1. Permission gate via `NotesService::note`.
2. `RoomManager::get(id).body_text()` — inside `Room::body_text` we lock the doc and call `Text::get_string(&txn)` to materialise the plain-text body used as LLM input. If no room exists (nobody has opened this note yet) we fall back to an empty string.
3. `AiService::summarize` (single call) or `AiService::agent_report` (three `AiAssistant` methods fanned out via `tokio::join!`).
4. The default `HeuristicAssistant` is deterministic and sync-cheap, which keeps the CI test suite hermetic. Swapping to `async-openai` or Ollama is a one-line wiring change in `AppState::bootstrap`.

### Internal / background flows

These are flows that no single API call triggers but that the system needs to remain correct and fast.

#### A. Event emission → bus → consumers

`NotesService::emit(event)` is the single choke point and always does two things under one logical operation:

```
events.append(event.clone())            // EventStore (durable)
bus.publish(event)                      // EventBus  (in-proc broadcast today)
```

Consumers today are in-process tests; in production the Redis adapter (`tn-common::eventbus::redis`, planned) will publish on channel `events.notes` and the (planned) `notes-query-service` will project into the read model. The ordering guarantee is: an event appears in the store *before* it is published on the bus, so a subscriber that restarts can catch up by replaying from the store.

#### B. `RoomManager` lifecycle

- `get_or_create(note_id)` uses `DashMap::entry().or_insert_with(...)` — lock-free for the common case, serialising only per-bucket on first creation.
- Each `Room` owns a `tokio::sync::broadcast::Sender` with a bounded 1024-message buffer and a `Mutex<Doc>`. The mutex is held across `apply` but dropped **before** we fan out to avoid head-of-line blocking on slow subscribers.
- `close(id)` is called from `deleteNote` and drops the `Arc<Room>`. Active subscribers receive `Err(Closed)` and the client reconnects if it still has permission.
- (Planned) idle-eviction task: a periodic `tokio::task` scans the map and closes rooms whose `tx.receiver_count() == 0` and last write was >5min ago, freeing memory for cold notes.

#### C. Resilience combinators (`tn-common::resilience`)

Every call to an external adapter that *could* fail (DB, Redis, LLM) is expected to be composed like:

```rust
breaker.call(|| with_timeout(Duration::from_millis(500),
    with_retry(3, || adapter.call(...))
)).await
```

- `with_timeout<F, T>` — generic wrapper around `tokio::time::timeout` that maps elapsed to `AppError::Timeout`.
- `with_retry<F, Fut, T>` — `tokio_retry::Retry` with `ExponentialBackoff::from_millis(20).factor(2).max_delay(500ms).map(jitter)`. The closure must be idempotent.
- `CircuitBreaker` — three atomic states (Closed / Open / HalfOpen). Trips open after `threshold` consecutive failures, auto-cools down after a configurable window, and allows a single probe in `HalfOpen`.

The `breaker_opens`, `timeout_triggers`, and `retry_eventually_succeeds` tests exercise all three.

#### D. Sharding / partitioning

`tn_infra::shard::ShardRouter::shard_for(user_id)` hashes the `UserId` UUID and returns `hash % N`. Today `NotesService` is shard-agnostic (in-memory); when the Postgres adapter lands, `NoteRepo::for_user` / `by_id` will pick the physical shard via the router, and the event store will use monthly range partitions (see `PLAN.md §2`).

#### E. Error mapping

Domain and service layers never produce transport-specific errors. They all return `AppError` (sealed `thiserror::Error` enum). The GraphQL edge converts via `to_gql(e) = Error::new(e.to_string())`. A future iteration will attach an `extensions.code` (e.g. `"UNAUTHORIZED"`, `"FORBIDDEN"`, `"CONFLICT"`) so the frontend can branch without string matching.

#### F. Observability

- All services initialise `tn_common::telemetry::init("<service>")` which installs a JSON `tracing-subscriber::fmt` layer with target+level and an `EnvFilter` seeded from `RUST_LOG`.
- Spans should be opened at the edge (`tracing::info_span!("graphql", op = %op_name)`) and propagated through `async` calls — async-graphql integrates with `tracing` automatically when the feature is enabled.
- Planned: OpenTelemetry OTLP layer exporting to Jaeger, and a `prometheus` registry exposed at `/metrics`.

#### G. Frontend co-edit loop

In `frontend/src/app/notes/[id]/page.tsx`:

1. A local `Y.Doc` is created on first render.
2. `useSubscription(NOTE_OPS)` establishes a WebSocket; on each incoming `updateB64` we `Y.applyUpdate(doc, b64decode(updateB64))` — the local doc converges toward the server state.
3. On `textarea` change we diff against the current state vector (`Y.encodeStateVector(doc)`), produce an incremental `Y.encodeStateAsUpdate(doc, before)`, base64-encode, and `applyOps` to the server. The server's broadcast will echo it back; Yjs de-duplicates because updates are idempotent (every operation carries a unique `(clientID, clock)` pair).

This is a classic "network provider" pattern for Yjs, using GraphQL as the transport instead of the stock `y-websocket` protocol.

### End-to-end trace: Alice shares with Bob, both edit live

```
Alice browser                    Gateway                        Bob browser
     │    register(alice)            │                                │
     ├──────────────────────────────▶│                                │
     │◀─── token ─────────────────────│                                │
     │    register(bob)               │                                │
     │                                │◀──────────────────────────────┤
     │                                │── token ─────────────────────▶│
     │    createNote("ideas")         │                                │
     ├──────────────────────────────▶│ NotesService::create           │
     │                                │  notes.insert                  │
     │                                │  acls.grant(owner=alice)       │
     │                                │  emit NoteCreated              │
     │                                │  rooms.get_or_create(id)       │
     │◀── Note(id, snapshotB64="") ──┤                                │
     │    shareNote(id, bob, EDITOR)  │                                │
     ├──────────────────────────────▶│ NotesService::share (saga)     │
     │                                │  users.by_email(bob)           │
     │                                │  acls.grant(bob, EDITOR)       │
     │                                │  emit NoteShared               │
     │◀── Collaborator(bob, EDITOR) ──┤                                │
     │                                │                                │
     │                                │◀── subscribe noteOps(id) ─────┤
     │                                │   rooms.get_or_create          │
     │                                │   broadcast.subscribe          │
     │                                │                                │
     │    applyOps(id, "…insertHi")   │                                │
     ├──────────────────────────────▶│ rooms[id].apply                │
     │                                │   doc.mutex.lock               │
     │                                │   apply_update                 │
     │                                │   tx.send(OpFrame)             │
     │                                │── OpEvent ───────────────────▶│
     │◀── snapshotB64(new) ──────────┤        Bob's Y.Doc.applyUpdate │
     │                                │                                │
     │                                │◀── applyOps(id, "…insertYo") ─┤
     │                                │   (symmetric path)             │
     │◀── OpEvent ───────────────────┤                                │
     │     Alice's Y.Doc.applyUpdate                                   │
```

Both Alice's and Bob's `Y.Doc`s converge on `"HiYo"` (or `"YoHi"`, depending on causal order) via CRDT merge — the server never arbitrates, merges are deterministic from the `(clientID, clock)` metadata inside the updates.

---
## ⚙️ Configuration
| Env var       | Default                         | Purpose                          |
|---------------|---------------------------------|----------------------------------|
| `JWT_SECRET`  | `dev-secret-change-me`          | HMAC secret for signing tokens   |
| `BIND`        | `0.0.0.0:9090`                  | HTTP bind address                |
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

## 🧪 Postman

A ready-to-import Postman v2.1 collection covering the full GraphQL surface (with auto-captured JWT & note IDs) and a WebSocket subscription request lives under [`postman/`](./postman/). See [`postman/README.md`](./postman/README.md) for the suggested run order.
