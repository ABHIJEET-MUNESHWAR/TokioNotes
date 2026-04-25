//! Generic repository / unit-of-work contracts. Keeping persistence
//! abstractions in `common` allows domain code to stay pure and testable.

use crate::error::AppResult;
use async_trait::async_trait;

#[async_trait]
pub trait Repository<T, ID>: Send + Sync
where
    T: Send + Sync,
    ID: Send + Sync,
{
    async fn get(&self, id: ID) -> AppResult<Option<T>>;
    async fn save(&self, entity: &T) -> AppResult<()>;
    async fn delete(&self, id: ID) -> AppResult<()>;
}
