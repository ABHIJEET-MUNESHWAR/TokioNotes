//! Notes command/query service. CQRS-lite: command handlers append domain
//! events to the event store and emit them on a generic event bus that the
//! collab and query consumers subscribe to.

use chrono::Utc;
use std::sync::Arc;
use tn_common::error::{AppError, AppResult};
use tn_common::eventbus::EventBus;
use tn_common::ids::{EventId, NoteId, UserId};
use tn_domain::events::DomainEvent;
use tn_domain::note::{EditSession, Idle, Note, NoteAcl, Role};
use tn_infra::repos::{AclRepo, EventStore, NoteRepo, UserRepo};

/// Generic over every collaborator so the same struct works in tests
/// (in-memory) and production (Postgres + Redis).
pub struct NotesService<U, N, A, E, B>
where
    U: UserRepo + 'static,
    N: NoteRepo + 'static,
    A: AclRepo + 'static,
    E: EventStore + 'static,
    B: EventBus<DomainEvent> + 'static,
{
    pub users: Arc<U>,
    pub notes: Arc<N>,
    pub acls: Arc<A>,
    pub events: Arc<E>,
    pub bus: Arc<B>,
}

impl<U, N, A, E, B> Clone for NotesService<U, N, A, E, B>
where
    U: UserRepo, N: NoteRepo, A: AclRepo, E: EventStore, B: EventBus<DomainEvent>,
{
    fn clone(&self) -> Self {
        Self {
            users: self.users.clone(),
            notes: self.notes.clone(),
            acls: self.acls.clone(),
            events: self.events.clone(),
            bus: self.bus.clone(),
        }
    }
}

impl<U, N, A, E, B> NotesService<U, N, A, E, B>
where
    U: UserRepo, N: NoteRepo, A: AclRepo, E: EventStore, B: EventBus<DomainEvent>,
{
    pub fn new(users: Arc<U>, notes: Arc<N>, acls: Arc<A>, events: Arc<E>, bus: Arc<B>) -> Self {
        Self { users, notes, acls, events, bus }
    }

    async fn emit(&self, evt: DomainEvent) -> AppResult<()> {
        self.events.append(evt.clone()).await?;
        self.bus.publish(evt).await
    }

    async fn require_role(&self, note: NoteId, user: UserId) -> AppResult<(Note, Role)> {
        let n = self
            .notes
            .by_id(note)
            .await?
            .ok_or_else(|| AppError::NotFound(note.to_string()))?;
        if n.deleted {
            return Err(AppError::NotFound("deleted".into()));
        }
        let role = if n.owner_id == user {
            Role::Owner
        } else {
            self.acls
                .role_of(note, user)
                .await?
                .ok_or_else(|| AppError::Forbidden("no access".into()))?
        };
        Ok((n, role))
    }

    pub async fn create(&self, owner: UserId, title: String) -> AppResult<Note> {
        let n = Note::create(owner, &title)?;
        self.notes.insert(&n).await?;
        self.acls
            .grant(&NoteAcl { note_id: n.id, user_id: owner, role: Role::Owner, granted_at: Utc::now() })
            .await?;
        self.emit(DomainEvent::NoteCreated {
            id: EventId::new(),
            note_id: n.id,
            owner_id: owner,
            title: n.title.clone(),
            at: Utc::now(),
        })
        .await?;
        Ok(n)
    }

    pub async fn rename(&self, actor: UserId, note: NoteId, title: String) -> AppResult<Note> {
        let (n, role) = self.require_role(note, actor).await?;
        let session = EditSession::<Idle>::open(n, actor, role)?.rename(title)?.commit();
        self.notes.update(&session.note).await?;
        self.emit(DomainEvent::NoteRenamed {
            id: EventId::new(),
            note_id: note,
            title: session.note.title.clone(),
            at: Utc::now(),
        })
        .await?;
        Ok(session.note)
    }

    pub async fn delete(&self, actor: UserId, note: NoteId) -> AppResult<()> {
        let (n, role) = self.require_role(note, actor).await?;
        if role != Role::Owner {
            return Err(AppError::Forbidden("only owner can delete".into()));
        }
        self.notes.delete(n.id).await?;
        self.emit(DomainEvent::NoteDeleted { id: EventId::new(), note_id: note, at: Utc::now() })
            .await?;
        Ok(())
    }

    /// Saga-style share: lookup user by email → grant ACL → emit event.
    /// On failure of the second step the caller could compensate by
    /// reverting the lookup; here it's idempotent and side-effect-free.
    pub async fn share(
        &self,
        actor: UserId,
        note: NoteId,
        with_email: &str,
        role: Role,
    ) -> AppResult<NoteAcl> {
        let (_, my_role) = self.require_role(note, actor).await?;
        if !my_role.can_share() {
            return Err(AppError::Forbidden("only owner can share".into()));
        }
        let target = self
            .users
            .by_email(with_email)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("no user {with_email}")))?;
        if target.id == actor {
            return Err(AppError::Validation("cannot share with yourself".into()));
        }
        let acl = NoteAcl { note_id: note, user_id: target.id, role, granted_at: Utc::now() };
        self.acls.grant(&acl).await?;
        self.emit(DomainEvent::NoteShared {
            id: EventId::new(),
            note_id: note,
            with_user: target.id,
            role,
            at: Utc::now(),
        })
        .await?;
        Ok(acl)
    }

    pub async fn revoke(&self, actor: UserId, note: NoteId, user: UserId) -> AppResult<()> {
        let (_, my_role) = self.require_role(note, actor).await?;
        if !my_role.can_share() {
            return Err(AppError::Forbidden("only owner can revoke".into()));
        }
        self.acls.revoke(note, user).await?;
        self.emit(DomainEvent::NoteShareRevoked {
            id: EventId::new(),
            note_id: note,
            user,
            at: Utc::now(),
        })
        .await?;
        Ok(())
    }

    pub async fn note(&self, actor: UserId, note: NoteId) -> AppResult<Note> {
        let (n, _) = self.require_role(note, actor).await?;
        Ok(n)
    }

    /// Permission gate for live CRDT writes (`applyOps`). Requires the
    /// caller to have at least `Editor` access; viewers are rejected.
    pub async fn note_for_edit(&self, actor: UserId, note: NoteId) -> AppResult<Note> {
        let (n, role) = self.require_role(note, actor).await?;
        if !role.can_edit() {
            return Err(AppError::Forbidden("viewer cannot edit".into()));
        }
        Ok(n)
    }

    pub async fn list_for(&self, user: UserId) -> AppResult<Vec<Note>> {
        let mut owned = self.notes.for_user(user).await?;
        let shared = self.acls.notes_for_user(user).await?;
        for nid in shared {
            if owned.iter().any(|n| n.id == nid) { continue; }
            if let Some(n) = self.notes.by_id(nid).await? {
                if !n.deleted { owned.push(n); }
            }
        }
        owned.retain(|n| !n.deleted);
        owned.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
        Ok(owned)
    }

    pub async fn collaborators(&self, actor: UserId, note: NoteId) -> AppResult<Vec<NoteAcl>> {
        let _ = self.require_role(note, actor).await?;
        self.acls.collaborators(note).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tn_common::eventbus::InProcBus;
    use tn_infra::repos::{InMemoryAclRepo, InMemoryEventStore, InMemoryNoteRepo, InMemoryUserRepo};
    use tn_domain::user::User;

    type Svc = NotesService<InMemoryUserRepo, InMemoryNoteRepo, InMemoryAclRepo, InMemoryEventStore, InProcBus<DomainEvent>>;

    async fn fixture() -> (Svc, UserId, UserId) {
        let users = Arc::new(InMemoryUserRepo::default());
        let notes = Arc::new(InMemoryNoteRepo::default());
        let acls = Arc::new(InMemoryAclRepo::default());
        let events = Arc::new(InMemoryEventStore::default());
        let bus = Arc::new(InProcBus::<DomainEvent>::new(64));
        let alice = User::builder().email("a@x").display_name("A").password_hash("h").build().unwrap();
        let bob = User::builder().email("b@x").display_name("B").password_hash("h").build().unwrap();
        users.create(&alice).await.unwrap();
        users.create(&bob).await.unwrap();
        let svc = NotesService::new(users, notes, acls, events, bus);
        (svc, alice.id, bob.id)
    }

    #[tokio::test]
    async fn create_share_revoke() {
        let (svc, alice, bob) = fixture().await;
        let n = svc.create(alice, "T".into()).await.unwrap();
        svc.share(alice, n.id, "b@x", Role::Editor).await.unwrap();
        assert_eq!(svc.list_for(bob).await.unwrap().len(), 1);
        svc.revoke(alice, n.id, bob).await.unwrap();
        assert_eq!(svc.list_for(bob).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn viewer_cannot_rename() {
        let (svc, alice, bob) = fixture().await;
        let n = svc.create(alice, "T".into()).await.unwrap();
        svc.share(alice, n.id, "b@x", Role::Viewer).await.unwrap();
        assert!(svc.rename(bob, n.id, "x".into()).await.is_err());
    }

    #[tokio::test]
    async fn viewer_cannot_apply_ops() {
        let (svc, alice, bob) = fixture().await;
        let n = svc.create(alice, "T".into()).await.unwrap();
        svc.share(alice, n.id, "b@x", Role::Viewer).await.unwrap();
        // Viewer is allowed to *read* the note…
        assert!(svc.note(bob, n.id).await.is_ok());
        // …but rejected by the live-edit gate.
        assert!(svc.note_for_edit(bob, n.id).await.is_err());
        // Editors and owners pass.
        svc.share(alice, n.id, "b@x", Role::Editor).await.unwrap();
        assert!(svc.note_for_edit(bob, n.id).await.is_ok());
        assert!(svc.note_for_edit(alice, n.id).await.is_ok());
    }

    #[tokio::test]
    async fn editor_can_rename() {
        let (svc, alice, bob) = fixture().await;
        let n = svc.create(alice, "T".into()).await.unwrap();
        svc.share(alice, n.id, "b@x", Role::Editor).await.unwrap();
        let n2 = svc.rename(bob, n.id, "X".into()).await.unwrap();
        assert_eq!(n2.title, "X");
        assert_eq!(n2.version, 1);
    }

    #[tokio::test]
    async fn only_owner_can_delete() {
        let (svc, alice, bob) = fixture().await;
        let n = svc.create(alice, "T".into()).await.unwrap();
        svc.share(alice, n.id, "b@x", Role::Editor).await.unwrap();
        assert!(svc.delete(bob, n.id).await.is_err());
        svc.delete(alice, n.id).await.unwrap();
        assert_eq!(svc.list_for(alice).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn cannot_share_with_self() {
        let (svc, alice, _) = fixture().await;
        let n = svc.create(alice, "T".into()).await.unwrap();
        assert!(svc.share(alice, n.id, "a@x", Role::Editor).await.is_err());
    }

    #[tokio::test]
    async fn share_unknown_email_404s() {
        let (svc, alice, _) = fixture().await;
        let n = svc.create(alice, "T".into()).await.unwrap();
        assert!(svc.share(alice, n.id, "ghost@x", Role::Editor).await.is_err());
    }
}

