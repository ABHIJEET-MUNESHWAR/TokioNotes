//! Shard routing: hash(user_id) % N. Demonstrates horizontal partitioning
//! intent even when the underlying store is in-memory.

use std::hash::{Hash, Hasher};
use tn_common::ids::UserId;

#[derive(Clone)]
pub struct ShardRouter {
    shards: usize,
}

impl ShardRouter {
    pub fn new(shards: usize) -> Self {
        assert!(shards > 0, "shards must be > 0");
        Self { shards }
    }
    pub fn shard_for(&self, user: UserId) -> usize {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        user.into_uuid().hash(&mut h);
        (h.finish() as usize) % self.shards
    }
    pub fn shards(&self) -> usize {
        self.shards
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic() {
        let r = ShardRouter::new(8);
        let u = UserId::new();
        assert_eq!(r.shard_for(u), r.shard_for(u));
        assert!(r.shard_for(u) < 8);
    }
}
