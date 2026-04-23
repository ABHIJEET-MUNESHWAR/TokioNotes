//! In-memory implementations of every repository. They use `dashmap` for
//! interior mutability without locking the entire store and are safe to
//! share across Tokio tasks. A `postgres` feature can later swap these
//! for `sqlx`-backed equivalents without changing service code.

use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use std::sync::Arc;
use tn_common::error::{AppError, AppResult};
use tn_common::ids::{NoteId, UserId};
use tn_domain::events::DomainEvent;
use tn_domain::note::{Note, NoteAcl, Role};
use tn_domain::user::User;

#[async_trait]
pub trait UserRepo: Send + Sync {
    async fn create(&self, u: &User) -> AppResult<()>;
    async fn by_email(&self, email: &str) -> AppResult<Option<User>>;
    async fn by_id(&self, id: UserId) -> AppResult<Option<User>>;
}

#[async_trait]
pub trait NoteRepo: Send + Sync {
    async fn insert(&self, n: &Note) -> AppResult<()>;
    async fn update(&self, n: &Note) -> AppResult<()>;
    async fn by_id(&self, id: NoteId) -> AppResult<Option<Note>>;
    async fn for_user(&self, user: UserId) -> AppResult<Vec<Note>>;
    async fn delete(&self, id: NoteId) -> AppResult<()>;
}

#[async_trait]
pub trait AclRepo: Send + Sync {
    async fn grant(&self, acl: &NoteAcl) -> AppResult<()>;
    async fn revoke(&self, note: NoteId, user: UserId) -> AppResult<()>;
    async fn role_of(&self, note: NoteId, user: UserId) -> AppResult<Option<Role>>;
    async fn collaborators(&self, note: NoteId) -> AppResult<Vec<NoteAcl>>;
    async fn notes_for_user(&self, user: UserId) -> AppResult<Vec<NoteId>>;
}

#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(&self, event: DomainEvent) -> AppResult<()>;
    async fn list(&self) -> AppResult<Vec<DomainEvent>>;
}

// ---------- In-memory implementations ---------------------------------------

#[derive(Default, Clone)]
pub struct InMemoryUserRepo {
    by_id: Arc<DashMap<UserId, User>>,
    by_email: Arc<DashMap<String, UserId>>,
}

#[async_trait]
impl UserRepo for InMemoryUserRepo {
    async fn create(&self, u: &User) -> AppResult<()> {
        if self.by_email.contains_key(&u.email) {
            return Err(AppError::Conflict("email already registered".into()));
        }
        self.by_email.insert(u.email.clone(), u.id);
        self.by_id.insert(u.id, u.clone());
        Ok(())
    }
    async fn by_email(&self, email: &str) -> AppResult<Option<User>> {
        Ok(self
            .by_email
            .get(&email.to_lowercase())
            .and_then(|id| self.by_id.get(&*id).map(|v| v.clone())))
    }
    async fn by_id(&self, id: UserId) -> AppResult<Option<User>> {
        Ok(self.by_id.get(&id).map(|v| v.clone()))
    }
}

#[derive(Default, Clone)]
pub struct InMemoryNoteRepo {
    notes: Arc<DashMap<NoteId, Note>>,
    by_owner: Arc<DashMap<UserId, Vec<NoteId>>>,
}

#[async_trait]
impl NoteRepo for InMemoryNoteRepo {
    async fn insert(&self, n: &Note) -> AppResult<()> {
        self.notes.insert(n.id, n.clone());
        self.by_owner.entry(n.owner_id).or_default().push(n.id);
        Ok(())
    }
    async fn update(&self, n: &Note) -> AppResult<()> {
        // Optimistic concurrency: refuse stale writes.
        if let Some(existing) = self.notes.get(&n.id) {
            if existing.version > n.version {
                return Err(AppError::Conflict("stale write".into()));
            }
        }
        self.notes.insert(n.id, n.clone());
        Ok(())
    }
    async fn by_id(&self, id: NoteId) -> AppResult<Option<Note>> {
        Ok(self.notes.get(&id).map(|v| v.clone()))
    }
    async fn for_user(&self, user: UserId) -> AppResult<Vec<Note>> {
        let ids = self.by_owner.get(&user).map(|v| v.clone()).unwrap_or_default();
        Ok(ids.into_iter().filter_map(|id| self.notes.get(&id).map(|v| v.clone())).collect())
    }
    async fn delete(&self, id: NoteId) -> AppResult<()> {
        if let Some(mut entry) = self.notes.get_mut(&id) {
            entry.deleted = true;
            entry.updated_at = Utc::now();
            return Ok(());
        }
        Err(AppError::NotFound(id.to_string()))
    }
}

#[derive(Default, Clone)]
pub struct InMemoryAclRepo {
    by_note: Arc<DashMap<NoteId, Vec<NoteAcl>>>,
    by_user: Arc<DashMap<UserId, Vec<NoteId>>>,
}

#[async_trait]
impl AclRepo for InMemoryAclRepo {
    async fn grant(&self, acl: &NoteAcl) -> AppResult<()> {
        let mut entry = self.by_note.entry(acl.note_id).or_default();
        entry.retain(|a| a.user_id != acl.user_id);
        entry.push(acl.clone());
        let mut un = self.by_user.entry(acl.user_id).or_default();
        if !un.contains(&acl.note_id) {
            un.push(acl.note_id);
        }
        Ok(())
    }
    async fn revoke(&self, note: NoteId, user: UserId) -> AppResult<()> {
        if let Some(mut e) = self.by_note.get_mut(&note) {
            e.retain(|a| a.user_id != user);
        }
        if let Some(mut e) = self.by_user.get_mut(&user) {
            e.retain(|n| *n != note);
        }
        Ok(())
    }
    async fn role_of(&self, note: NoteId, user: UserId) -> AppResult<Option<Role>> {
        Ok(self
            .by_note
            .get(&note)
            .and_then(|v| v.iter().find(|a| a.user_id == user).map(|a| a.role)))
    }
    async fn collaborators(&self, note: NoteId) -> AppResult<Vec<NoteAcl>> {
        Ok(self.by_note.get(&note).map(|v| v.clone()).unwrap_or_default())
    }
    async fn notes_for_user(&self, user: UserId) -> AppResult<Vec<NoteId>> {
        Ok(self.by_user.get(&user).map(|v| v.clone()).unwrap_or_default())
    }
}

#[derive(Default, Clone)]
pub struct InMemoryEventStore {
    events: Arc<DashMap<u64, DomainEvent>>,
    next: Arc<std::sync::atomic::AtomicU64>,
}

#[async_trait]
impl EventStore for InMemoryEventStore {
    async fn append(&self, event: DomainEvent) -> AppResult<()> {
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.events.insert(n, event);
        Ok(())
    }
    async fn list(&self) -> AppResult<Vec<DomainEvent>> {
        let mut keys: Vec<u64> = self.events.iter().map(|e| *e.key()).collect();
        keys.sort();
        Ok(keys.into_iter().filter_map(|k| self.events.get(&k).map(|v| v.clone())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tn_domain::user::User;

    #[tokio::test]
    async fn user_repo_unique_email() {
        let r = InMemoryUserRepo::default();
        let u = User::builder().email("a@b.com").display_name("A").password_hash("h").build().unwrap();
        r.create(&u).await.unwrap();
        let dup = User::builder().email("a@b.com").display_name("B").password_hash("h").build().unwrap();
        assert!(r.create(&dup).await.is_err());
    }

    #[tokio::test]
    async fn acl_grant_revoke() {
        let r = InMemoryAclRepo::default();
        let nid = NoteId::new();
        let uid = UserId::new();
        r.grant(&NoteAcl { note_id: nid, user_id: uid, role: Role::Editor, granted_at: Utc::now() }).await.unwrap();
        assert_eq!(r.role_of(nid, uid).await.unwrap(), Some(Role::Editor));
        r.revoke(nid, uid).await.unwrap();
        assert_eq!(r.role_of(nid, uid).await.unwrap(), None);
    }
}

