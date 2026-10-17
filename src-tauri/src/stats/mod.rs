//! Prefill / decode statistics.
//!
//! Principles:
//! - live prefill/decode t/s is a sliding-window rate (per-slot `/slots`
//!   snapshots over 10 s), summed over active slots; the last value is
//!   held while a slot is still in phase (no `--` flicker), `None`
//!   only when truly idle/stale;
//! - averages are computed over reads (polls), not over
//!   requests: each poll with an observed speed feeds the average;
//! - the per-request history is stored on the backend side (memory only,
//!   last 50) and returned on every poll: the frontend only
//!   displays it, no accumulation/dedupe on the React side;
//! - in router mode every model is tracked independently (own slots,
//!   counters, averages, history rows);
//! - no disk persistence.
//!
//! The module tree:
//! - [`types`]    shared data model (samples, tracks, payloads)
//! - [`prom`]     `/metrics` Prometheus parsing
//! - [`rate`]     sliding-window rate maths (pure)
//! - [`log`]      server log parsing, attributed per model instance
//! - [`sample`]   building one finished-request sample
//! - [`endpoint`] router HTTP layer (`?model=…&autoload=false`)
//! - [`poll`]     the live-tracking algorithm, applied to one model
//! - [`activity`] the poll command: drives `poll` for every model

// `pub` on the three modules holding `#[tauri::command]`s: `generate_handler!`
// resolves `stats::cmds::send_test_prompt` and needs the path reachable.
pub mod activity;
pub mod cmds;
pub(crate) mod endpoint;
mod log;
mod poll;
pub mod prom;
mod rate;
mod sample;
mod types;

// `StatsState` is the Tauri-managed state: `lib.rs` needs the type itself.
// Everything else is reached through its own submodule.
pub use types::StatsState;

#[cfg(test)]
mod tests;
