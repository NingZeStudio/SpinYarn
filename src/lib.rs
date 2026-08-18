//! SpinYarn Web API binary crate. The deobfuscation core lives in the
//! `spinyarn-core` crate; this crate re-exports it for integration tests and
//! layers the Axum HTTP API on top.

pub mod api;
pub mod error;

// Re-export the core engine so `spinyarn::mapping::...` / `spinyarn::deobfuscator::...`
// paths keep working for tests, benches, and downstream embedders.
pub use spinyarn_core::{cache, config, deobfuscator, mapping, Spinyarn};

use std::sync::atomic::AtomicU64;

pub static START_TIME: AtomicU64 = AtomicU64::new(0);
