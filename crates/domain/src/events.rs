//! Domain events. CQRS write side appends; read side projects.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tn_common::prelude::*;

use crate::note::Role;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DomainEvent {
    UserRegistered {
        id: EventId,
        user_id: UserId,
        email: String,
        at: DateTime<Utc>,
    },
    NoteCreated {
        id: EventId,
        note_id: NoteId,
        owner_id: UserId,
        title: String,
        at: DateTime<Utc>,
    },
    NoteRenamed {
        id: EventId,
        note_id: NoteId,
        title: String,
        at: DateTime<Utc>,
    },
    NoteShared {
        id: EventId,
        note_id: NoteId,
        /// User who performed the share (typically the owner).
        actor: UserId,
        /// Recipient of the new ACL grant.
        with_user: UserId,
        role: Role,
        at: DateTime<Utc>,
    },
    NoteShareRevoked {
        id: EventId,
        note_id: NoteId,
        user: UserId,
        at: DateTime<Utc>,
    },
    NoteDeleted {
        id: EventId,
        note_id: NoteId,
        at: DateTime<Utc>,
    },
    NoteOpsApplied {
        id: EventId,
        note_id: NoteId,
        actor: UserId,
        seq: i64,
        at: DateTime<Utc>,
    },
}

impl DomainEvent {
    pub fn id(&self) -> EventId {
        match self {
            DomainEvent::UserRegistered { id, .. }
            | DomainEvent::NoteCreated { id, .. }
            | DomainEvent::NoteRenamed { id, .. }
            | DomainEvent::NoteShared { id, .. }
            | DomainEvent::NoteShareRevoked { id, .. }
            | DomainEvent::NoteDeleted { id, .. }
            | DomainEvent::NoteOpsApplied { id, .. } => *id,
        }
    }
}
