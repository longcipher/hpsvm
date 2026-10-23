//! The SVM type family: one trait whose associated types describe every
//! swappable axis of the VM.
//!
//! The design intent is that a fork changes one trait impl and inherits the
//! whole machinery. Extension slots default to the unit type, so an unused
//! extension point is zero-sized and monomorphizes away; nothing that does not
//! read a slot pays for it.
//!
//! This mirrors the `EvmTypesHost` family of the reference project: the
//! associated types are the spec identifier, the transaction envelope, the
//! six extension slots, and the host itself.
//!
//! # Deferred bounds
//!
//! None remain: `type SpecId: Copy + Into<ClusterSpecId>` is expressed now that
//! [`Version`](crate::Version) provides [`ClusterSpecId`].

use solana_transaction::versioned::VersionedTransaction;

use crate::{ClusterSpecId, Svm, host::Host};

/// One associated-type family describing every swappable axis of the VM.
///
/// A type implementing this trait is a *type family*: it selects the spec
/// identifier, the transaction envelope, the six extension slots and the host
/// the rest of the workspace talks to. Everything downstream is written once
/// against the trait and instantiated per family.
pub trait SvmTypesHost: Sized + 'static {
    /// The spec identifier describing the active protocol profile.
    type SpecId: Copy + Into<ClusterSpecId>;
    /// The transaction envelope the VM accepts.
    type Tx;
    /// Block-environment extension state.
    ///
    /// The VM stores this value inside [`BlockEnv`](crate::BlockEnv), hands it
    /// out by value on every host access, and shares the VM across Rayon batch
    /// workers, so it must stay small, copyable, debuggable,
    /// default-constructible and thread-shareable. The base family sets it to
    /// `()`.
    type BlockEnvExt: Clone + Copy + std::fmt::Debug + Default + Send + Sync;
    /// The host this family runs against.
    type Host<'a>: Host<Self> + ?Sized;
}

/// Blanket marker for a family whose host is the VM itself.
///
/// Requiring `for<'a> SvmTypesHost<Host<'a> = Svm<'a, Self>>` is what makes "the
/// host is this VM" a type-system fact rather than a runtime convention: a type
/// can only satisfy this marker by pointing its host associated type at the VM
/// parameterized by itself.
pub trait SvmTypes: for<'a> SvmTypesHost<Host<'a> = Svm<'a, Self>> {}

/// The base type family: every extension slot is the unit type, except the
/// spec identifier, which is the [`ClusterSpecId`][crate::ClusterSpecId] the
/// VM's [`Version`][crate::Version] tracks.
///
/// This is the family the rest of the workspace instantiates through the
/// [`Hpsvm`](crate::Hpsvm) alias, and the one a fork starts from before
/// replacing the slots it cares about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BaseSvmTypes;

impl SvmTypesHost for BaseSvmTypes {
    type SpecId = ClusterSpecId;
    type Tx = VersionedTransaction;
    type BlockEnvExt = ();
    // Deferred: `Host<Self> + ?Sized` once the typed host boundary exists.
    type Host<'a> = Svm<'a, Self>;
}

impl SvmTypes for BaseSvmTypes {}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use solana_transaction::versioned::VersionedTransaction;

    use super::*;

    /// Every extension slot of the base family must be zero-sized, so an
    /// unused extension point disappears from the generated code. The spec-id
    /// slot is the exception: it names a [`ClusterSpecId`][crate::ClusterSpecId]
    /// (`repr(u32)`, 4 bytes), not the unit type.
    #[test]
    fn base_svm_types_extension_slots_are_zero_sized() {
        use crate::ClusterSpecId;
        assert_eq!(size_of::<BaseSvmTypes>(), 0);
        assert_eq!(size_of::<<BaseSvmTypes as SvmTypesHost>::SpecId>(), size_of::<ClusterSpecId>());
        assert_eq!(size_of::<<BaseSvmTypes as SvmTypesHost>::SpecId>(), 4);
        assert_eq!(size_of::<<BaseSvmTypes as SvmTypesHost>::BlockEnvExt>(), 0);
    }

    /// The envelope slot is the one non-unit associated type: it must stay the
    /// transaction type the rest of the workspace already threads through.
    #[test]
    fn base_svm_types_selects_the_versioned_transaction_envelope() {
        let envelope: Option<<BaseSvmTypes as SvmTypesHost>::Tx> = None::<VersionedTransaction>;
        assert!(envelope.is_none());
    }

    /// The marker can only be satisfied by a family that points its host at the
    /// VM itself, which is what keeps `Host<'a>` from drifting to a look-alike.
    #[test]
    fn base_svm_types_satisfies_the_blanket_marker() {
        fn assert_marker<T: SvmTypes>() {}
        assert_marker::<BaseSvmTypes>();
    }

    /// The base host is exactly the type the `Hpsvm` alias names, so the
    /// pre-existing public-behaviour suite keeps compiling unchanged.
    #[test]
    fn the_base_host_is_the_compatibility_alias() {
        fn same_host(host: Svm<'static, BaseSvmTypes>) -> crate::Hpsvm {
            host
        }

        let aliased: crate::Hpsvm = same_host(Svm::<BaseSvmTypes>::new());
        assert_eq!(aliased.latest_blockhash(), crate::Hpsvm::new().latest_blockhash());
    }
}
