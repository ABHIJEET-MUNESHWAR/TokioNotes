//! CLI: print the gateway's GraphQL SDL to stdout.
//!
//! Used to regenerate `graphql/schema.graphql` when the Rust schema in
//! `crates/gateway/src/schema.rs` changes. A unit test
//! (`schema_sdl_matches_disk`) asserts the on-disk file matches what
//! this binary would produce, so drift is caught in CI.
//!
//! Usage:
//!     cargo run -p tn-gateway --bin dump-schema > graphql/schema.graphql

use tn_gateway::app::AppState;
use tn_gateway::schema::build_schema;

fn main() {
    // The SDL is purely a function of the Rust type definitions; the
    // concrete `AppState` we attach is irrelevant. Use the in-memory
    // bootstrap so this binary doesn't need a database.
    let st = AppState::bootstrap("dump-schema-secret");
    let schema = build_schema(st);
    print!("{}{}", tn_gateway::schema::SDL_HEADER, schema.sdl());
}
