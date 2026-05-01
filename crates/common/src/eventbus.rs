//! Generic, in-process event bus built on `tokio::sync::broadcast`.
//! Adapters (e.g. Redis) implement the same `EventBus` trait so transports
//! are pluggable per the Open/Closed principle.

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::Stream;
use tokio_stream::StreamExt;

use crate::error::{AppError, AppResult};

#[async_trait]
pub trait EventBus<E>: Send + Sync + 'static
where
    E: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    async fn publish(&self, event: E) -> AppResult<()>;
    fn subscribe(&self) -> std::pin::Pin<Box<dyn Stream<Item = AppResult<E>> + Send>>;
}

#[derive(Clone)]
pub struct InProcBus<E: Clone + Send + Sync + 'static> {
    tx: broadcast::Sender<E>,
}

impl<E: Clone + Send + Sync + 'static> InProcBus<E> {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }
    pub fn sender(&self) -> broadcast::Sender<E> {
        self.tx.clone()
    }
}

impl<E: Clone + Send + Sync + 'static> Default for InProcBus<E> {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[async_trait]
impl<E> EventBus<E> for InProcBus<E>
where
    E: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    async fn publish(&self, event: E) -> AppResult<()> {
        // `send` only errors when there are zero subscribers; that's fine.
        let _ = self.tx.send(event);
        Ok(())
    }
    fn subscribe(&self) -> std::pin::Pin<Box<dyn Stream<Item = AppResult<E>> + Send>> {
        let rx = self.tx.subscribe();
        let s = BroadcastStream::new(rx).map(|r| r.map_err(|e| AppError::Internal(e.to_string())));
        Box::pin(s)
    }
}

pub type SharedBus<E> = Arc<InProcBus<E>>;

// ----- Storage-agnostic enum wrapper ---------------------------------------
//
// Lets the gateway pick a transport at boot time (in-process for tests &
// single-replica deploys, Redis pub/sub for multi-replica) without making
// `NotesService` generic-bound proliferate at every call site.

#[derive(Clone)]
pub enum AnyBus<E>
where
    E: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    InProc(InProcBus<E>),
    #[cfg(feature = "redis")]
    Redis(crate::redis_bus::RedisBus<E>),
}

impl<E> Default for AnyBus<E>
where
    E: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    fn default() -> Self {
        Self::InProc(InProcBus::default())
    }
}

#[async_trait]
impl<E> EventBus<E> for AnyBus<E>
where
    E: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    async fn publish(&self, event: E) -> AppResult<()> {
        match self {
            Self::InProc(b) => b.publish(event).await,
            #[cfg(feature = "redis")]
            Self::Redis(b) => b.publish(event).await,
        }
    }
    fn subscribe(&self) -> std::pin::Pin<Box<dyn Stream<Item = AppResult<E>> + Send>> {
        match self {
            Self::InProc(b) => b.subscribe(),
            #[cfg(feature = "redis")]
            Self::Redis(b) => b.subscribe(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
    struct Evt(u32);

    #[tokio::test]
    async fn pubsub_round_trip() {
        let bus: InProcBus<Evt> = InProcBus::new(16);
        let mut sub = bus.subscribe();
        bus.publish(Evt(1)).await.unwrap();
        let got = sub.next().await.unwrap().unwrap();
        assert_eq!(got, Evt(1));
    }
}
