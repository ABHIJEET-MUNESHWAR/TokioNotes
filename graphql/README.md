# GraphQL schema

The TokioNotes API is **code-first**: the source of truth lives in
[`crates/gateway/src/schema.rs`](../crates/gateway/src/schema.rs), where
`async-graphql` derive macros (`#[Object]`, `#[Subscription]`,
`SimpleObject`, …) produce the runtime schema from Rust types.

This folder mirrors that schema as **GraphQL SDL** so it's easy to:

- read the API surface without a Rust toolchain
- import into Postman / Insomnia / Apollo Studio for code-gen
- diff API changes in pull-request reviews

## Files

| File | What |
|---|---|
| `schema.graphql` | Full SDL for the gateway (Query, Mutation, Subscription, all types and scalars). |

## How it stays in sync

A unit test in the gateway crate (`schema_sdl_matches_disk`) compares
the on-disk SDL to what the live schema would produce on every
`cargo test` run. CI fails if you change the Rust schema without
updating the file.

### Regenerate after a schema change

Either run the binary:

```bash
cargo run -p tn-gateway --bin dump-schema > graphql/schema.graphql
```

…or let the test rewrite the file in place:

```bash
BLESS_SDL=1 cargo test -p tn-gateway schema_sdl_matches_disk
```

Then commit `graphql/schema.graphql` alongside your Rust change.

