//! The map and set types the crate uses internally.
//!
//! The `hashbrown` feature swaps the standard library's maps for
//! `hashbrown`'s, whose default hasher (foldhash) is markedly cheaper than
//! the standard library's SipHash. The account overlay is the hottest map in
//! the crate, but the transaction registry, the precompile registry, the
//! batch planner's sets and the diagnostics' mint map are consulted on the
//! transaction path too, so they all follow the same switch.
//!
//! The types are re-exported rather than defined here so that call sites
//! read as plain `HashMap` / `HashSet`; only this module carries the
//! conditional.

#[cfg(not(feature = "hashbrown"))]
pub use std::collections::{HashMap, HashSet};

#[cfg(feature = "hashbrown")]
pub use hashbrown::{HashMap, HashSet};
