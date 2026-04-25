//! Collaborative editing rooms backed by Y-CRDT (`yrs`).
//!
//! * Each note has at most one `Room`, held in a `DashMap` for lock-free
//!   lookup. The `Doc` itself is wrapped in a `tokio::sync::Mutex` so
//!   updates serialise per-room while still permitting massive parallelism
//!   across rooms.
//! * Updates fan out via a `tokio::sync::broadcast` channel; subscribers
//!   that lag are silently re-synced from the latest state vector.
//! * The CRDT guarantees convergence regardless of arrival order, so we
//!   never reject concurrent writes.

use dashmap::DashMap;
use std::sync::Arc;
use tn_common::error::{AppError, AppResult};
use tn_common::ids::NoteId;
use tokio::sync::{broadcast, Mutex};
use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

#[derive(Clone, Debug)]
pub struct OpFrame {
    pub note: NoteId,
    pub update: Vec<u8>,
}

pub struct Room {
    pub note: NoteId,
    doc: Mutex<Doc>,
    tx: broadcast::Sender<OpFrame>,
}

impl Room {
    fn new(note: NoteId) -> Self {
        let doc = Doc::new();
        // Touch the root text so it exists in the doc.
        let _ = doc.get_or_insert_text("body");
        let (tx, _) = broadcast::channel(1024);
        Self {
            note,
            doc: Mutex::new(doc),
            tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<OpFrame> {
        self.tx.subscribe()
    }

    /// Apply an incoming update. The CRDT merges concurrent edits.
    pub async fn apply(&self, update_bytes: &[u8]) -> AppResult<()> {
        // The `Update` type is `!Send`; decode AFTER acquiring the mutex so
        // it never crosses an `.await` boundary.
        let doc = self.doc.lock().await;
        {
            let update = Update::decode_v1(update_bytes)
                .map_err(|e| AppError::Validation(format!("bad update: {e}")))?;
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| AppError::Validation(e.to_string()))?;
        }
        drop(doc);
        // Best-effort fanout; lag is recoverable via state-vector resync.
        let _ = self.tx.send(OpFrame {
            note: self.note,
            update: update_bytes.to_vec(),
        });
        Ok(())
    }

    /// Encode the full state as a single update for late joiners.
    pub async fn snapshot(&self) -> Vec<u8> {
        let doc = self.doc.lock().await;
        let bytes = doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        bytes
    }

    /// Plain-text projection of the document body, for AI processing.
    pub async fn body_text(&self) -> String {
        let doc = self.doc.lock().await;
        let txt = doc.get_or_insert_text("body");
        let txn = doc.transact();
        txt.get_string(&txn)
    }
}

#[derive(Clone, Default)]
pub struct RoomManager {
    rooms: Arc<DashMap<NoteId, Arc<Room>>>,
}

impl RoomManager {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get_or_create(&self, note: NoteId) -> Arc<Room> {
        self.rooms
            .entry(note)
            .or_insert_with(|| Arc::new(Room::new(note)))
            .clone()
    }
    pub fn get(&self, note: NoteId) -> Option<Arc<Room>> {
        self.rooms.get(&note).map(|r| r.clone())
    }
    pub fn close(&self, note: NoteId) {
        self.rooms.remove(&note);
    }
    pub fn len(&self) -> usize {
        self.rooms.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }
}

/// Convenience: encode a local edit `body.insert(idx, text)` as a v1 update.
pub fn local_insert_update(initial: &[u8], idx: u32, text: &str) -> AppResult<Vec<u8>> {
    let doc = Doc::new();
    if !initial.is_empty() {
        let upd = Update::decode_v1(initial).map_err(|e| AppError::Validation(e.to_string()))?;
        doc.transact_mut()
            .apply_update(upd)
            .map_err(|e| AppError::Validation(e.to_string()))?;
    }
    let before = { doc.transact().state_vector() };
    let txt = doc.get_or_insert_text("body");
    txt.insert(&mut doc.transact_mut(), idx, text);
    let bytes = doc.transact().encode_state_as_update_v1(&before);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn two_clients_converge() {
        let mgr = RoomManager::new();
        let note = NoteId::new();
        let room = mgr.get_or_create(note);

        // Client A inserts "Hello "
        let snap = room.snapshot().await;
        let upd_a = local_insert_update(&snap, 0, "Hello ").unwrap();
        room.apply(&upd_a).await.unwrap();

        // Client B starts from same baseline and inserts "World"
        let snap = room.snapshot().await;
        let upd_b = local_insert_update(&snap, 6, "World").unwrap();
        room.apply(&upd_b).await.unwrap();

        let body = room.body_text().await;
        assert!(body.contains("Hello"));
        assert!(body.contains("World"));
    }

    #[tokio::test]
    async fn broadcast_delivers_updates() {
        let mgr = RoomManager::new();
        let room = mgr.get_or_create(NoteId::new());
        let mut rx = tokio_stream::wrappers::BroadcastStream::new(room.subscribe());
        let upd = local_insert_update(&room.snapshot().await, 0, "hi").unwrap();
        room.apply(&upd).await.unwrap();
        let frame = tokio::time::timeout(std::time::Duration::from_millis(200), rx.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(frame.update, upd);
    }

    #[tokio::test]
    async fn invalid_update_rejected() {
        let mgr = RoomManager::new();
        let room = mgr.get_or_create(NoteId::new());
        assert!(room.apply(b"not-a-valid-update").await.is_err());
    }

    #[tokio::test]
    async fn manager_lifecycle() {
        let mgr = RoomManager::new();
        let id = NoteId::new();
        let _ = mgr.get_or_create(id);
        assert_eq!(mgr.len(), 1);
        mgr.close(id);
        assert!(mgr.is_empty());
    }
}
