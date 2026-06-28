//! Hand-written `GlobalDispatch`/`Dispatch` glue for Sayuki's first-party
//! protocols, layered over the generated `sayuki-protocols` bindings.
//!
//! The generated crate stays bindings-only; the handlers live here so they can
//! reach `SayukiState`, mirroring the `screencopy.rs` precedent for
//! `wlr-screencopy`. Each protocol gets its own submodule.

pub(crate) mod project;

pub(crate) use project::{ProjectAffinity, ProjectManagerState};
