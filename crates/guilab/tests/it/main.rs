//! End-to-end tests: drive the real `guilab` binary against real apps.
//! Each test gets its own server via a private runtime dir. Tests needing an app that
//! isn't installed are skipped (`GUILAB_REQUIRE_APPS=1` makes that a failure). Shared
//! helpers, and how to wait in a test, are in `harness.rs`.

mod harness;

mod apps;
mod cleanup;
mod events;
mod images;
mod input;
mod sessions;
mod viewer;
mod xwayland;

pub use harness::*;
