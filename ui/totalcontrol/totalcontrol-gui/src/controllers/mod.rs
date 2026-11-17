//! Generic MIDI controller support: one engine, data-only definition files.
//!
//! A controller is described by a JSON definition (`map.rs`), found through
//! the search order of `load.rs`, run by the shared engine (`engine.rs`).
//! Mackie Control is the first shipped definition; the RME ARC USB joins
//! with its tested-support pull request. The format and the engine contract
//! are `docs/spec/TOTALCONTROL-CONTROLLER-SPEC.md`.

pub mod discover;
pub mod engine;
pub mod expr;
pub mod load;
pub mod map;

#[cfg(test)]
mod parity;