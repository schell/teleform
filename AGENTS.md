# AGENTS.md — Teleform

## Project Overview

Teleform is a Rust Infrastructure-as-Code (IaC) library using DAG-based CRUD
scheduling. It is a Cargo workspace with two crates:

- **`teleform`** (lib name: **`tele`**) — core library with `Resource` trait, `Store`, `Remote`/`RemoteVar` types
- **`teleform-derive`** — proc-macro crate providing `#[derive(HasDependencies)]`

Rust edition: 2021. No feature flags. No CI pipeline. No pinned toolchain.

## Build / Lint / Test Commands

```sh
# Build the entire workspace
cargo build

# Build a single crate
cargo build -p teleform
cargo build -p teleform-derive

# Run all tests
cargo test

# Run tests for a single crate
cargo test -p teleform

# Run a single test by name
cargo test -p teleform sanity            # the main integration test
cargo test -p teleform migrate_ser       # a specific unit test

# Run tests with logging output
RUST_LOG=trace cargo test -p teleform -- --nocapture

# Check (no codegen, faster)
cargo check --workspace

# Lint with clippy (no config file — uses defaults)
cargo clippy --workspace -- -D warnings

# Format check (no rustfmt.toml — uses defaults)
cargo fmt --all -- --check

# Format in-place
cargo fmt --all
```

There are **no Makefiles, justfiles, or shell scripts** — use standard `cargo` commands.

## Workspace Layout

```
Cargo.toml                            # workspace root (resolver = "2")
.cargo/config.toml                    # sets CARGO_WORKSPACE_DIR env var
crates/
  teleform/
    src/
      lib.rs                          # core types: Resource, Store, Error, Action, etc. (~1200 lines)
      remote.rs                       # Remote<X>, RemoteVar<T>, Migrated<T>
      has_dependencies_impl.rs        # blanket HasDependencies impls for primitives/collections/tuples
      utils.rs                        # sha256_digest helper
      test.rs                         # main integration test (included via #[cfg(test)] mod test)
  teleform-derive/
    src/
      lib.rs                          # #[derive(HasDependencies)] and impl_has_dependencies_tuples!
```

**Important:** The library crate is named `tele`, not `teleform`. In downstream
code and tests, you write `use tele::*` or `use crate::{self as tele, *}`.

## Code Style

### Imports

Organize imports into groups separated by blank lines:

```rust
// 1. Standard library
use std::{future::Future, ops::Deref, pin::Pin};

// 2. External crates
use dagga::{dot::DagLegend, Node, Schedule};
use snafu::prelude::*;

// 3. Re-exports and module declarations
pub use teleform_derive::HasDependencies;
mod has_dependencies_impl;
pub mod remote;

// 4. Internal imports (crate:: or super::)
use remote::{Migrated, Remote, RemoteVar, Remotes};
```

- Use `crate::` for top-level items, `super::` for sibling items in the same module.
- Wildcard imports (`use crate::{self as tele, *}`) are acceptable in test and impl files only.
- `snafu::prelude::*` is the one external wildcard import (per snafu convention).

### Error Handling

The crate uses **`snafu`** as the primary error framework with `anyhow` for leaf/utility functions.

- The main error type is the `Error` enum in `lib.rs` deriving `snafu::Snafu`.
- Each variant has `#[snafu(display("..."))]` with a descriptive message.
- A module-level alias exists: `type Result<T, E = Error> = core::result::Result<T, E>;`
- Use `.context(FooSnafu { field: value })?` for error propagation.
- Use `snafu::ensure!(condition, FooSnafu { ... })` for precondition checks.
- Use `.fail()` to generate errors without a source: `MissingNameSnafu { missing }.fail()`
- `anyhow::Result` is acceptable for small utility functions (see `utils.rs`).
- `anyhow::Error` is bridged into the main `Error` via the `Tele` variant + `From<anyhow::Error>`.
- User-facing resource errors use the `UserError` marker trait (`Display + Debug + 'static`).

### Async Patterns

- Runtime: **tokio** with `features = ["full"]`.
- `Resource` trait methods use **RPITIT** (`impl Future<Output = ...>`) in the trait definition.
- Default trait method bodies use `unimplemented!()` intentionally — this is a design choice.
- Implementors use `async fn` directly.
- Type-erased async closures: `Pin<Box<dyn Future<Output = Result<()>> + '_>>`.
- Use `tokio::fs` for file operations and `tokio::process::Command` for subprocesses.

### Naming Conventions

- **Types:** `PascalCase` — `StoreResource`, `RemoteVar`, `InertStoreResource`
- **Functions/methods:** `snake_case` — `define_resource`, `dequeue_var`, `sha256_digest`
- **Modules:** `snake_case` — `remote`, `has_dependencies_impl`, `utils`
- **Resource pairs:** `Local*` / `Remote*` prefixes — `LocalBucket` / `RemoteBucket`
- **Error contexts:** `*Snafu` suffix — `SerializeSnafu`, `LoadSnafu`, `MissingResourceSnafu`
- **Serde intermediaries:** `*Proxy` suffix — `RemoteProxy`, `MigratedProxy`
- **Abbreviations:** `rez` for "resource key" (dagga concept)
- **Generic params:** Single uppercase letters (`T`, `P`, `X`, `L`, `R`); `Provider` for the provider role

### Derives and Serde

The standard derive set for data structs:

```rust
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
```

- Always use **fully qualified** `serde::Serialize` / `serde::Deserialize` in derives.
- `HasDependencies` derive is added when a struct has fields that are themselves dependencies.
- Serde attributes: `#[serde(untagged)]`, `#[serde(try_from = "...")]` as needed.

### Documentation

- **Module-level:** `//!` doc comments at the top of each file with markdown headings.
- **Public API:** `///` doc comments on traits, methods, and key types. Use `## Note` and `## Errors` subsections.
- **Unsafe/unwrap justifications:** `// UNWRAP: safe because ...` comment pattern.
- **Inline comments:** For non-obvious logic only; keep them concise.

### Formatting

- **No rustfmt.toml** — default `rustfmt` settings apply (4-space indent, ~100 col soft limit).
- Trailing commas in multi-line constructs (struct literals, args, match arms, derive lists).
- Method chains: one `.method()` per line when spanning multiple lines, indented 4 spaces.
- Opening brace on same line as declaration.
- `#[allow(unreachable_code)]` on the `Resource` trait (suppresses `unimplemented!()` warnings).

### Logging

Uses the `log` crate. All levels are used:

- `log::trace!` — fine-grained debug info (remote resolution, file contents)
- `log::debug!` — operational details (store loading, scheduling)
- `log::info!` — major operations (create/update/delete actions, saving state)
- `log::warn!` — non-fatal warnings (missing resources, fallback behavior)
- `log::error!` — errors that need attention

### Tests

- Tests live in `#[cfg(test)] mod test` blocks (sometimes as a separate file like `test.rs`).
- Async tests use `#[tokio::test]`.
- Initialize logging with `let _ = env_logger::builder().try_init();`.
- Use `.unwrap()` liberally in tests (not `.context()`/`?`).
- Mock resources are hand-written structs with `Provider = ()` — no mock framework.
- Test output goes to `test_output/` (gitignored) using `CARGO_WORKSPACE_DIR` env var.
- `pretty_assertions` is a dependency but is used in the library itself for change diffing.

### Store Workflow

The `Store` uses a plan/apply two-step pattern:

```rust
let mut store = Store::new("state/", provider);
store.register::<MyResource>();      // register types for orphan auto-delete
let res = store.resource("id", MyResource { ... })?;
let plan = store.plan()?;            // scan disk, detect orphans, build schedule
store.apply(plan).await?;            // execute the plan
```

- `store.plan()` consumes the DAG — call `get_schedule_string()` / `save_apply_graph()` **before** `plan()`.
- `store.register::<T>()` enables auto-deletion of orphaned resources of type `T`.
- `store.pending_destroy::<T>(id)` handles orphans that need dependency migration.
