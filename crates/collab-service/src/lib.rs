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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;
use tn_common::error::{AppError, AppResult};
use tn_common::ids::NoteId;
use tn_infra::repos::{AnySnapshotStore, SnapshotStore};
use tokio::sync::{broadcast, Mutex, Notify, OnceCell};
use tokio::time::Instant;
use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

/// Quiet period after the last edit before we flush a snapshot to the
/// store. Tuned for human typing: most pauses (>500 ms) collapse a burst
/// of keystrokes into a single Postgres write.
const DEBOUNCE_QUIET: Duration = Duration::from_millis(500);

/// Hard upper bound between a dirty mark and a flush. Caps write
/// amplification for *continuous* typing (where the quiet period would
/// otherwise never trigger) at one Postgres upsert every 5 seconds.
const DEBOUNCE_MAX_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct OpFrame {
    pub note: NoteId,
    pub update: Vec<u8>,
}

#[derive(Copy, Clone, Debug)]
struct Pending {
    /// Earliest of (last_op + QUIET) and (first_op + MAX_WAIT).
    deadline: Instant,
    /// When the room first transitioned from clean → dirty in this cycle.
    first_dirty_at: Instant,
}

pub struct Room {
    pub note: NoteId,
    doc: Mutex<Doc>,
    tx: broadcast::Sender<OpFrame>,
    store: Arc<AnySnapshotStore>,
    /// Resolves once we've replayed the persisted snapshot into `doc`.
    /// Loading is async and `RoomManager::get_or_create` is sync, so we
    /// hydrate lazily on first read/write — `OnceCell::get_or_try_init`
    /// guarantees it runs at most once even with concurrent callers.
    loaded: OnceCell<()>,
    /// Coalescing buffer for snapshot persistence. `None` = no pending
    /// write; `Some(p)` = flush by `p.deadline`. A `std::sync::Mutex` is
    /// fine here — the critical section is a few ns and never crosses
    /// an `.await`, which also keeps `mark_dirty` callable from any
    /// context (sync or async).
    dirty: StdMutex<Option<Pending>>,
    /// Wakes the per-room flusher task whenever `dirty` is updated.
    notify: Notify,
    /// Set by `RoomManager::close` to tell the flusher task to exit even
    /// if there's a still-strong reference held by an in-flight handler.
    shutdown: AtomicBool,
}

impl Room {
    /// Allocate a room and spawn its background snapshot-flusher task.
    /// The task holds a `Weak<Room>` so it exits naturally when the
    /// `RoomManager` evicts the entry — no manual cancellation token
    /// required.
    fn new(note: NoteId, store: Arc<AnySnapshotStore>) -> Arc<Self> {
        let doc = Doc::new();
        // Touch the root text so it exists in the doc.
        let _ = doc.get_or_insert_text("body");
        let (tx, _) = broadcast::channel(1024);
        let room = Arc::new(Self {
            note,
            doc: Mutex::new(doc),
            tx,
            store,
            loaded: OnceCell::new(),
            dirty: StdMutex::new(None),
            notify: Notify::new(),
            shutdown: AtomicBool::new(false),
        });
        tokio::spawn(Room::flusher(Arc::downgrade(&room)));
        room
    }

    pub fn subscribe(&self) -> broadcast::Receiver<OpFrame> {
        self.tx.subscribe()
    }

    async fn ensure_loaded(&self) -> AppResult<()> {
        self.loaded
            .get_or_try_init(|| async {
                let Some(state) = self.store.latest(self.note).await? else {
                    return Ok::<(), AppError>(());
                };
                let doc = self.doc.lock().await;
                {
                    // `Update` is `!Send` — keep it inside a single sync
                    // scope that holds the lock; never cross `.await`.
                    let upd = Update::decode_v1(&state)
                        .map_err(|e| AppError::Validation(format!("snapshot decode: {e}")))?;
                    let mut txn = doc.transact_mut();
                    txn.apply_update(upd)
                        .map_err(|e| AppError::Validation(e.to_string()))?;
                }
                drop(doc);
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Apply an incoming update. The CRDT merges concurrent edits.
    /// Persistence is *not* synchronous: we mark the room dirty and let
    /// the per-room flusher task batch saves under the configured
    /// debounce. Live peers still see the update immediately via the
    /// broadcast fanout.
    pub async fn apply(&self, update_bytes: &[u8]) -> AppResult<()> {
        self.ensure_loaded().await?;
        {
            // The `Update` type is `!Send`; decode AFTER acquiring the
            // mutex so it never crosses an `.await` boundary.
            let doc = self.doc.lock().await;
            let update = Update::decode_v1(update_bytes)
                .map_err(|e| AppError::Validation(format!("bad update: {e}")))?;
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| AppError::Validation(e.to_string()))?;
        }
        self.mark_dirty();
        // Best-effort fanout; lag is recoverable via state-vector resync.
        let _ = self.tx.send(OpFrame {
            note: self.note,
            update: update_bytes.to_vec(),
        });
        Ok(())
    }

    /// Mark the room dirty and (re)compute the flush deadline. Idempotent
    /// and lock-free w.r.t. the doc; intended to be called from `apply`.
    fn mark_dirty(&self) {
        let now = Instant::now();
        let mut slot = self.dirty.lock().expect("dirty lock poisoned");
        let first_dirty_at = slot.map(|p| p.first_dirty_at).unwrap_or(now);
        let cap = first_dirty_at + DEBOUNCE_MAX_WAIT;
        let deadline = std::cmp::min(now + DEBOUNCE_QUIET, cap);
        *slot = Some(Pending {
            deadline,
            first_dirty_at,
        });
        drop(slot);
        // Wake the flusher so it can recompute its sleep.
        self.notify.notify_one();
    }

    /// Force an immediate snapshot persist if the room is dirty.
    /// Called by `RoomManager::close` so we don't lose buffered state
    /// when a note is deleted; safe to invoke from anywhere.
    pub async fn flush(&self) -> AppResult<()> {
        let was_dirty = self.dirty.lock().expect("dirty lock poisoned").is_some();
        if !was_dirty {
            return Ok(());
        }
        let state = {
            let doc = self.doc.lock().await;
            let bytes = doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
            bytes
        };
        match self.store.save(self.note, &state).await {
            Ok(()) => {
                // Only clear the dirty flag on success so a transient
                // store error doesn't silently lose the buffered state —
                // the next `apply` (or the next flusher tick) will retry.
                *self.dirty.lock().expect("dirty lock poisoned") = None;
                Ok(())
            }
            Err(e) => {
                tracing::error!(note=%self.note, error=%e, "snapshot persist failed");
                Err(e)
            }
        }
    }

    /// Background loop driving debounced persistence. Exits when the
    /// owning `Arc<Room>` is dropped (`Weak::upgrade()` returns `None`)
    /// or when `RoomManager::close` flips the `shutdown` flag — the
    /// latter handles the case where an in-flight request still holds a
    /// strong ref while the manager has evicted the entry.
    async fn flusher(weak: Weak<Self>) {
        loop {
            let Some(room) = weak.upgrade() else {
                return;
            };
            if room.shutdown.load(Ordering::Acquire) {
                return;
            }

            let next = *room.dirty.lock().expect("dirty lock poisoned");
            match next {
                None => {
                    // Clean — block until first dirty mark (or shutdown).
                    room.notify.notified().await;
                }
                Some(p) => {
                    let now = Instant::now();
                    if now < p.deadline {
                        // Wake on either the deadline or a new dirty
                        // mark — the latter lets ongoing typing push
                        // the deadline back without flushing prematurely.
                        tokio::select! {
                            _ = tokio::time::sleep_until(p.deadline) => {}
                            _ = room.notify.notified() => {}
                        }
                    }
                    if room.shutdown.load(Ordering::Acquire) {
                        return;
                    }
                    // Re-check: a notify between snapshot and flush may
                    // have moved the deadline forward — only flush when
                    // we've truly crossed it.
                    let due = room
                        .dirty
                        .lock()
                        .expect("dirty lock poisoned")
                        .map(|p| Instant::now() >= p.deadline)
                        .unwrap_or(false);
                    if due {
                        let _ = room.flush().await;
                    }
                }
            }
        }
    }

    /// Encode the full state as a single update for late joiners.
    pub async fn snapshot(&self) -> Vec<u8> {
        if let Err(e) = self.ensure_loaded().await {
            tracing::error!(note=%self.note, error=%e, "snapshot load failed");
        }
        let doc = self.doc.lock().await;
        let bytes = doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        bytes
    }

    /// Plain-text projection of the document body, for AI processing.
    pub async fn body_text(&self) -> String {
        if let Err(e) = self.ensure_loaded().await {
            tracing::error!(note=%self.note, error=%e, "snapshot load failed");
        }
        let doc = self.doc.lock().await;
        let txt = doc.get_or_insert_text("body");
        let txn = doc.transact();
        txt.get_string(&txn)
    }
}

#[derive(Clone)]
pub struct RoomManager {
    rooms: Arc<DashMap<NoteId, Arc<Room>>>,
    store: Arc<AnySnapshotStore>,
}

impl Default for RoomManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RoomManager {
    /// Build a manager backed by the in-memory snapshot store. Used by
    /// unit tests and single-node dev runs without `DATABASE_URL`.
    pub fn new() -> Self {
        Self::with_store(Arc::new(AnySnapshotStore::default()))
    }

    /// Build a manager that persists/loads document state through the
    /// supplied store (typically Postgres in production).
    pub fn with_store(store: Arc<AnySnapshotStore>) -> Self {
        Self {
            rooms: Arc::new(DashMap::new()),
            store,
        }
    }

    pub fn get_or_create(&self, note: NoteId) -> Arc<Room> {
        self.rooms
            .entry(note)
            .or_insert_with(|| Room::new(note, self.store.clone()))
            .clone()
    }
    pub fn get(&self, note: NoteId) -> Option<Arc<Room>> {
        self.rooms.get(&note).map(|r| r.clone())
    }
    /// Evict a room. Flushes any buffered snapshot first so we don't
    /// lose state when a note is deleted or the gateway shuts down,
    /// then signals the flusher task to exit.
    pub async fn close(&self, note: NoteId) {
        if let Some((_, room)) = self.rooms.remove(&note) {
            let _ = room.flush().await;
            room.shutdown.store(true, Ordering::Release);
            room.notify.notify_waiters();
        }
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
        mgr.close(id).await;
        assert!(mgr.is_empty());
    }

    /// A burst of edits within the quiet period must not yet have hit
    /// the store, and `RoomManager::close` must flush whatever is still
    /// buffered. Asserts the observable property against the in-memory
    /// store rather than counting calls (the `AnySnapshotStore` enum
    /// can't accept arbitrary mock impls).
    #[tokio::test]
    async fn debounce_coalesces_writes() {
        use tn_infra::repos::InMemorySnapshotStore;

        let store = InMemorySnapshotStore::default();
        let mgr = RoomManager::with_store(Arc::new(AnySnapshotStore::Mem(store.clone())));
        let id = NoteId::new();
        let room = mgr.get_or_create(id);

        // Apply 10 ops back-to-back, well inside the 500 ms quiet
        // window. None of them should have been persisted yet — the
        // per-room flusher is still waiting for the typing to pause.
        let snap = room.snapshot().await;
        for (cursor, ch) in ('a'..='j').enumerate() {
            let upd = local_insert_update(&snap, cursor as u32, &ch.to_string())
                .expect("local insert update");
            room.apply(&upd).await.unwrap();
        }
        assert_eq!(
            store.latest(id).await.unwrap(),
            None,
            "writes must be debounced — nothing should be persisted mid-burst"
        );

        // `close` drains the buffer synchronously so we never lose state
        // when a note is deleted or the gateway shuts down.
        mgr.close(id).await;
        assert!(
            store.latest(id).await.unwrap().is_some(),
            "close must flush buffered state"
        );
    }
}
