//! Infrastructure adapters. To keep the workspace buildable without a
//! running Postgres, we ship in-memory implementations of every repository
//! gated on the absence of the `postgres` cargo feature. The interfaces are
//! identical so business logic and tests are storage-agnostic.

pub mod auth;
pub mod password;
pub mod shard;
pub mod repos;

