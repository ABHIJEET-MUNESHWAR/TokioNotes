//! Cross-cutting primitives: errors, IDs, telemetry, resilience helpers,
//! generic repository contracts and an in-process event bus.
pub mod error;
pub mod ids;
pub mod telemetry;
pub mod resilience;
pub mod eventbus;
pub mod repository;
pub mod prelude;

