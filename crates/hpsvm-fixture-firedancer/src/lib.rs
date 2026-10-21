//! The FireDancer fixture codec for `hpsvm`.
//!
//! This crate adapts the FireDancer protobuf fixture format
//! ([`hpsvm-fd-fixture`]) to the `hpsvm` fixture model
//! ([`hpsvm_fixture`]). It is a separate crate so the protobuf dependency and
//! the vendored FireDancer fork stay out of the core fixture crate: a
//! consumer that only records and replays `hpsvm` fixtures never pulls them
//! in.
//!
//! # Example
//!
//! ```rust
//! use hpsvm_fixture_firedancer::FiredancerFixture;
//!
//! // The FireDancer codec exposes the fixture model it converts to.
//! let _type_name = std::any::type_name::<FiredancerFixture>();
//! ```

#![deny(rustdoc::broken_intra_doc_links)]

mod fd;

pub use crate::fd::{AdapterError, FiredancerFixture};
