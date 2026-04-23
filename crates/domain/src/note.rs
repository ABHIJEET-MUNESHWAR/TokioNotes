use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use tn_common::prelude::*;

/// ACL role. `Owner` ⊃ `Editor` ⊃ `Viewer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, async_graphql::Enum)]
pub enum Role {
    Viewer,
    Editor,
    Owner,
}

impl Role {
    pub fn can_edit(self) -> bool { matches!(self, Role::Editor | Role::Owner) }
    pub fn can_share(self) -> bool { matches!(self, Role::Owner) }
}

/// Note aggregate root. Mutation must go through `EditSession`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    pub owner_id: UserId,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
    pub deleted: bool,
}

impl Note {
    pub fn create(owner_id: UserId, title: impl Into<String>) -> AppResult<Self> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(AppError::Validation("title cannot be empty".into()));
        }
        let now = Utc::now();
        Ok(Self {
            id: NoteId::new(),
            owner_id,
            title,
            created_at: now,
            updated_at: now,
            version: 0,
            deleted: false,
        })
    }
}

/// ACL entry binding a user to a note with a role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteAcl {
    pub note_id: NoteId,
    pub user_id: UserId,
    pub role: Role,
    pub granted_at: DateTime<Utc>,
}

// ----- Typestate edit session ------------------------------------------------
pub struct Idle;
pub struct Editing;
pub struct Committed;

/// Compile-time enforced edit lifecycle: only `Editing` sessions can mutate
/// a note, and only `Editing` sessions can be committed.
pub struct EditSession<S> {
    pub note: Note,
    pub actor: UserId,
    pub role: Role,
    _state: PhantomData<S>,
}

impl EditSession<Idle> {
    pub fn open(note: Note, actor: UserId, role: Role) -> AppResult<EditSession<Editing>> {
        if !role.can_edit() {
            return Err(AppError::Forbidden("read-only access".into()));
        }
        if note.deleted {
            return Err(AppError::NotFound("note deleted".into()));
        }
        Ok(EditSession { note, actor, role, _state: PhantomData })
    }
}

impl EditSession<Editing> {
    pub fn rename(mut self, new_title: String) -> AppResult<Self> {
        if new_title.trim().is_empty() {
            return Err(AppError::Validation("title cannot be empty".into()));
        }
        self.note.title = new_title;
        self.note.updated_at = Utc::now();
        self.note.version += 1;
        Ok(self)
    }
    pub fn commit(self) -> EditSession<Committed> {
        EditSession { note: self.note, actor: self.actor, role: self.role, _state: PhantomData }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn role_capabilities() {
        assert!(Role::Owner.can_edit() && Role::Owner.can_share());
        assert!(Role::Editor.can_edit() && !Role::Editor.can_share());
        assert!(!Role::Viewer.can_edit());
    }
    #[test]
    fn cannot_open_session_as_viewer() {
        let n = Note::create(UserId::new(), "t").unwrap();
        let r = EditSession::<Idle>::open(n, UserId::new(), Role::Viewer);
        assert!(r.is_err());
    }
    #[test]
    fn rename_increments_version() {
        let owner = UserId::new();
        let n = Note::create(owner, "old").unwrap();
        let s = EditSession::<Idle>::open(n, owner, Role::Owner).unwrap();
        let s = s.rename("new".into()).unwrap();
        assert_eq!(s.note.title, "new");
        assert_eq!(s.note.version, 1);
        let _ = s.commit();
    }
    #[test]
    fn empty_title_rejected() {
        assert!(Note::create(UserId::new(), "").is_err());
    }
}

