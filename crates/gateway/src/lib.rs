//! Composition root: wires every service together into one Actix-Web
//! process exposing a single GraphQL endpoint (HTTP + WebSocket subs).
//! In production each crate would be deployed as its own service; this
//! binary remains useful as a local-dev all-in-one and as the canonical
//! integration-test entry point.

pub mod app;
pub mod schema;

