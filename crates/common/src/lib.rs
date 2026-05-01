//! Cross-cutting primitives: errors, IDs, telemetry, resilience helpers,
//! generic repository contracts and an in-process event bus.
pub mod error;
pub mod eventbus;
pub mod ids;
pub mod prelude;
#[cfg(feature = "redis")]
pub mod redis_bus;
pub mod repository;
pub mod resilience;
pub mod telemetry;
