use std::sync::Arc;
use tn_ai_agent_service::AiService;
use tn_auth_service::AuthService;
use tn_collab_service::RoomManager;
use tn_common::eventbus::InProcBus;
use tn_domain::ai::HeuristicAssistant;
use tn_domain::events::DomainEvent;
use tn_infra::auth::JwtIssuer;
use tn_infra::repos::{InMemoryAclRepo, InMemoryEventStore, InMemoryNoteRepo, InMemoryUserRepo};
use tn_notes_service::NotesService;

pub type Notes = NotesService<
    InMemoryUserRepo,
    InMemoryNoteRepo,
    InMemoryAclRepo,
    InMemoryEventStore,
    InProcBus<DomainEvent>,
>;
pub type Auth = AuthService<InMemoryUserRepo>;
pub type Ai = AiService<HeuristicAssistant>;

/// Bag of singletons shared across HTTP/GraphQL handlers.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<Auth>,
    pub notes: Arc<Notes>,
    pub rooms: RoomManager,
    pub ai: Arc<Ai>,
    pub bus: Arc<InProcBus<DomainEvent>>,
    pub users: Arc<InMemoryUserRepo>,
}

impl AppState {
    pub fn bootstrap(jwt_secret: &str) -> Self {
        let users = Arc::new(InMemoryUserRepo::default());
        let notes = Arc::new(InMemoryNoteRepo::default());
        let acls = Arc::new(InMemoryAclRepo::default());
        let events = Arc::new(InMemoryEventStore::default());
        let bus = Arc::new(InProcBus::<DomainEvent>::new(2048));

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
            rooms: RoomManager::new(),
            ai,
            bus,
            users,
        }
    }
}
