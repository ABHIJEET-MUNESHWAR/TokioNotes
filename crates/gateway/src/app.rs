use std::sync::Arc;
use tn_ai_agent_service::AiService;
use tn_auth_service::AuthService;
use tn_collab_service::RoomManager;
use tn_common::eventbus::{AnyBus, InProcBus};
use tn_domain::ai::HeuristicAssistant;
use tn_domain::events::DomainEvent;
use tn_infra::auth::JwtIssuer;
use tn_infra::repos::{
    AnyAclRepo, AnyEventStore, AnyNoteRepo, AnySnapshotStore, AnyUserRepo, InMemoryAclRepo,
    InMemoryEventStore, InMemoryNoteRepo, InMemorySnapshotStore, InMemoryUserRepo,
};
use tn_notes_service::NotesService;

pub type Notes =
    NotesService<AnyUserRepo, AnyNoteRepo, AnyAclRepo, AnyEventStore, AnyBus<DomainEvent>>;
pub type Auth = AuthService<AnyUserRepo>;
pub type Ai = AiService<HeuristicAssistant>;

/// Bag of singletons shared across HTTP/GraphQL handlers.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<Auth>,
    pub notes: Arc<Notes>,
    pub rooms: RoomManager,
    pub ai: Arc<Ai>,
    pub bus: Arc<AnyBus<DomainEvent>>,
    pub users: Arc<AnyUserRepo>,
}

impl AppState {
    pub fn bootstrap(jwt_secret: &str) -> Self {
        // Pick storage backends at startup. With `DATABASE_URL` set every
        // repository talks to Postgres; without it the gateway falls back
        // to the in-memory adapters used by unit tests. `connect_lazy`
        // keeps bootstrap synchronous and tolerates a transient DB hiccup.
        let backends = Backends::from_env();
        let users = Arc::new(backends.users);
        let notes = Arc::new(backends.notes);
        let acls = Arc::new(backends.acls);
        let events = Arc::new(backends.events);
        let snapshots = Arc::new(backends.snapshots);
        let bus = Arc::new(build_bus());

        let auth = Arc::new(AuthService::new(
            users.clone(),
            JwtIssuer::new(jwt_secret.as_bytes().to_vec(), 60 * 60 * 24),
        ));
        let notes_svc = Arc::new(NotesService::new(
            users.clone(),
            notes,
            acls,
            events,
            bus.clone(),
        ));
        let ai = Arc::new(AiService::new(Arc::new(HeuristicAssistant)));

        Self {
            auth,
            notes: notes_svc,
            rooms: RoomManager::with_store(snapshots),
            ai,
            bus,
            users,
        }
    }
}

/// Pick the event-bus transport. With `REDIS_URL` set, we use Redis
/// pub/sub so notification events (e.g. `NoteShared`) reach websocket
/// subscribers on every gateway replica. Without it, fall back to the
/// in-process broadcast — ideal for tests and single-node dev.
fn build_bus() -> AnyBus<DomainEvent> {
    if let Ok(url) = std::env::var("REDIS_URL") {
        if !url.is_empty() {
            match tn_common::redis_bus::RedisBus::<DomainEvent>::connect(
                &url,
                tn_common::redis_bus::DEFAULT_CHANNEL,
            ) {
                Ok(bus) => {
                    tracing::info!(target: "tn-gateway", "event bus: redis pubsub");
                    return AnyBus::Redis(bus);
                }
                Err(e) => {
                    tracing::error!(error=%e, "redis bus init failed, falling back to in-proc");
                }
            }
        }
    }
    tracing::warn!("REDIS_URL not set — using in-process event bus");
    AnyBus::InProc(InProcBus::<DomainEvent>::new(2048))
}

struct Backends {
    users: AnyUserRepo,
    notes: AnyNoteRepo,
    acls: AnyAclRepo,
    events: AnyEventStore,
    snapshots: AnySnapshotStore,
}

impl Backends {
    #[cfg(feature = "postgres")]
    fn from_env() -> Self {
        match std::env::var("DATABASE_URL") {
            Ok(url) if !url.is_empty() => match tn_infra::pg::pool_from_url(&url) {
                Ok(pool) => {
                    tracing::info!(target: "tn-gateway", "storage: postgres ({})", redact(&url));
                    Self {
                        users: AnyUserRepo::Pg(tn_infra::pg::PgUserRepo::new(pool.clone())),
                        notes: AnyNoteRepo::Pg(tn_infra::pg::PgNoteRepo::new(pool.clone())),
                        acls: AnyAclRepo::Pg(tn_infra::pg::PgAclRepo::new(pool.clone())),
                        events: AnyEventStore::Pg(tn_infra::pg::PgEventStore::new(pool.clone())),
                        snapshots: AnySnapshotStore::Pg(tn_infra::pg::PgSnapshotStore::new(pool)),
                    }
                }
                Err(e) => {
                    tracing::error!(error=%e, "failed to build pg pool, falling back to in-memory");
                    Self::in_memory()
                }
            },
            _ => {
                tracing::warn!("DATABASE_URL not set — using in-memory storage");
                Self::in_memory()
            }
        }
    }

    #[cfg(not(feature = "postgres"))]
    fn from_env() -> Self {
        Self::in_memory()
    }

    fn in_memory() -> Self {
        Self {
            users: AnyUserRepo::Mem(InMemoryUserRepo::default()),
            notes: AnyNoteRepo::Mem(InMemoryNoteRepo::default()),
            acls: AnyAclRepo::Mem(InMemoryAclRepo::default()),
            events: AnyEventStore::Mem(InMemoryEventStore::default()),
            snapshots: AnySnapshotStore::Mem(InMemorySnapshotStore::default()),
        }
    }
}

#[cfg(feature = "postgres")]
fn redact(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        if let Some((_, host)) = rest.split_once('@') {
            return format!("{scheme}://***@{host}");
        }
    }
    url.to_string()
}
