//! Redis pub/sub adapter for [`EventBus`].
//!
//! Why: when more than one gateway replica is running, an in-process
//! `tokio::sync::broadcast` only fans events out to subscribers in the
//! *same* process — a user holding a websocket on replica B will never
//! see a `NoteShared` event emitted on replica A. Routing every event
//! through Redis pub/sub fixes that with a tiny ops footprint (Redis is
//! already in the stack for caching/CRDT presence).
//!
//! Design:
//! * `publish` → `PUBLISH tn:events <json>` over a multiplexed connection
//!   (cheap, lock-free, and amortises TCP setup).
//! * A background bridge task subscribes to the channel and pushes every
//!   message into a local `tokio::sync::broadcast`. Subscribers consume
//!   from that local channel, so the hot path is identical to the
//!   in-process bus and lag handling is inherited for free.
//! * On disconnect the bridge reconnects with a small back-off; we never
//!   panic the gateway because of a transient Redis hiccup.

use async_trait::async_trait;
use futures::StreamExt as _;
use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{broadcast, OnceCell};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::Stream;

use crate::error::{AppError, AppResult};
use crate::eventbus::EventBus;

/// Default Redis pub/sub channel — every gateway replica subscribes to it.
pub const DEFAULT_CHANNEL: &str = "tn:events";

#[derive(Clone)]
pub struct RedisBus<E>
where
    E: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    client: redis::Client,
    channel: Arc<String>,
    local: broadcast::Sender<E>,
    publisher: Arc<OnceCell<redis::aio::ConnectionManager>>,
    _ph: PhantomData<fn() -> E>,
}

impl<E> RedisBus<E>
where
    E: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    /// Connect to Redis and start the pub/sub → local-broadcast bridge.
    /// `Client::open` is synchronous, so this is safe to call from
    /// non-async bootstrap code as long as a Tokio runtime is running
    /// (it `tokio::spawn`s the bridge task).
    pub fn connect(url: &str, channel: impl Into<String>) -> AppResult<Self> {
        let client =
            redis::Client::open(url).map_err(|e| AppError::Internal(format!("redis open: {e}")))?;
        let (tx, _rx) = broadcast::channel::<E>(2048);
        let bus = Self {
            client: client.clone(),
            channel: Arc::new(channel.into()),
            local: tx.clone(),
            publisher: Arc::new(OnceCell::new()),
            _ph: PhantomData,
        };
        let channel = bus.channel.clone();
        tokio::spawn(async move {
            run_bridge::<E>(client, channel.as_str().to_owned(), tx).await;
        });
        Ok(bus)
    }

    async fn publisher(&self) -> AppResult<redis::aio::ConnectionManager> {
        let mgr = self
            .publisher
            .get_or_try_init(|| async {
                redis::aio::ConnectionManager::new(self.client.clone())
                    .await
                    .map_err(|e| AppError::Internal(format!("redis pub conn: {e}")))
            })
            .await?;
        Ok(mgr.clone())
    }
}

async fn run_bridge<E>(client: redis::Client, channel: String, tx: broadcast::Sender<E>)
where
    E: Clone + Send + Sync + DeserializeOwned + 'static,
{
    let mut backoff_ms: u64 = 200;
    loop {
        match client.get_async_pubsub().await {
            Ok(mut ps) => {
                if let Err(e) = ps.subscribe(&channel).await {
                    tracing::error!(error=%e, channel=%channel, "redis subscribe failed");
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(5_000);
                    continue;
                }
                tracing::info!(channel=%channel, "redis pubsub bridge connected");
                backoff_ms = 200;
                let mut s = ps.on_message();
                while let Some(msg) = s.next().await {
                    let payload: String = match msg.get_payload::<String>() {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(error=%e, "redis payload decode");
                            continue;
                        }
                    };
                    match serde_json::from_str::<E>(&payload) {
                        Ok(ev) => {
                            // `send` only fails when there are zero local
                            // subscribers — perfectly fine.
                            let _ = tx.send(ev);
                        }
                        Err(e) => tracing::warn!(error=%e, "redis event deserialize"),
                    }
                }
                tracing::warn!(channel=%channel, "redis pubsub stream ended; reconnecting");
            }
            Err(e) => {
                tracing::error!(error=%e, "redis pubsub connect failed; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(5_000);
            }
        }
    }
}

#[async_trait]
impl<E> EventBus<E> for RedisBus<E>
where
    E: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    async fn publish(&self, event: E) -> AppResult<()> {
        let payload =
            serde_json::to_string(&event).map_err(|e| AppError::Internal(e.to_string()))?;
        let mut conn = self.publisher().await?;
        let _: i64 = redis::cmd("PUBLISH")
            .arg(self.channel.as_str())
            .arg(payload)
            .query_async(&mut conn)
            .await
            .map_err(|e| AppError::Internal(format!("redis publish: {e}")))?;
        Ok(())
    }

    fn subscribe(&self) -> Pin<Box<dyn Stream<Item = AppResult<E>> + Send>> {
        let rx = self.local.subscribe();
        let s = BroadcastStream::new(rx).map(|r| r.map_err(|e| AppError::Internal(e.to_string())));
        Box::pin(s)
    }
}
