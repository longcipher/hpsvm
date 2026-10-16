//! Per-instance precompile dispatch: which program ids execute as precompiles
//! and what runs for each of them.
//!
//! Agave owns precompile *execution*: [`InvokeContextCallback`] still drives
//! the call, but instead of consulting agave's global table it consults the
//! provider installed on the VM. A VM therefore runs the standard native set
//! by default, nothing at all with [`NoPrecompiles`], or a custom set that
//! shadows natives id by id.
//!
//! [`InvokeContextCallback`]: solana_svm_callback::InvokeContextCallback

use std::{collections::HashMap, fmt, marker::PhantomData, sync::Arc};

use agave_feature_set::FeatureSet;
use solana_address::Address;
use solana_precompile_error::PrecompileError;

use crate::SvmTypesHost;

/// A single precompile entry: the program id it handles plus its verifier.
///
/// The signature binds agave's real verify function
/// (`fn(&[u8], &[&[u8]], &FeatureSet) -> Result<(), PrecompileError>`).
/// There is deliberately no compute-unit or invocation-context parameter:
/// agave hands verifiers no context either, so a CU-aware precompile is
/// impossible on this path and the registry stays honest about it.
pub trait Precompile<T: SvmTypesHost>: Send + Sync {
    /// The program id this entry handles.
    fn program_id(&self) -> Address;

    /// Whether this entry is live under `feature_set`.
    ///
    /// The default is always-on. The native adapter overrides it with the
    /// entry's optional agave feature gate, so gating is enforced at lookup
    /// time exactly like agave's `get_precompile` feature check.
    fn is_enabled(&self, _feature_set: &FeatureSet) -> bool {
        true
    }

    /// Runs the precompile over `data` plus every instruction's data.
    fn verify(
        &self,
        data: &[u8],
        slices: &[&[u8]],
        feature_set: &FeatureSet,
    ) -> Result<(), PrecompileError>;
}

/// The active precompile set: program id to handler.
///
/// Inserting under an existing program id replaces (shadows) the previous
/// entry. Handlers are reference-counted, so cloning a set shares them.
pub struct PrecompileSet<T: SvmTypesHost> {
    entries: HashMap<Address, Arc<dyn Precompile<T>>>,
    // `fn() -> T` keeps the set `Send + Sync` for every family instead of
    // inheriting the auto traits of `T`.
    _marker: PhantomData<fn() -> T>,
}

impl<T: SvmTypesHost> PrecompileSet<T> {
    /// An empty active set: nothing dispatches as a precompile.
    pub fn new() -> Self {
        Self { entries: HashMap::new(), _marker: PhantomData }
    }

    /// Inserts `precompile` under its program id, returning the entry it
    /// shadows, if any.
    pub fn insert(&mut self, precompile: Arc<dyn Precompile<T>>) -> Option<Arc<dyn Precompile<T>>> {
        self.entries.insert(precompile.program_id(), precompile)
    }

    /// Returns the handler for `program_id`, if the set holds one.
    pub fn get(&self, program_id: &Address) -> Option<Arc<dyn Precompile<T>>> {
        self.entries.get(program_id).cloned()
    }

    /// Whether the set holds a handler for `program_id`.
    pub fn contains(&self, program_id: &Address) -> bool {
        self.entries.contains_key(program_id)
    }

    /// The number of handlers in the set.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the set holds no handlers.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every program id in the set, in no particular order.
    pub fn program_ids(&self) -> Vec<Address> {
        self.entries.keys().cloned().collect()
    }
}

impl<T: SvmTypesHost> Clone for PrecompileSet<T> {
    fn clone(&self) -> Self {
        Self { entries: self.entries.clone(), _marker: PhantomData }
    }
}

impl<T: SvmTypesHost> Default for PrecompileSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: SvmTypesHost> fmt::Debug for PrecompileSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrecompileSet").field("program_ids", &self.program_ids()).finish()
    }
}

/// Per-instance precompile dispatch, queried by program id.
///
/// The provider answers which program ids are live precompiles ([`get`](PrecompileProvider::get))
/// and enumerates them ([`program_ids`](PrecompileProvider::program_ids),
/// used for account installation). Every [`PrecompileSet`] is its own
/// provider; [`NoPrecompiles`] and (with the `precompiles` feature)
/// `NativePrecompiles` cover the standard shapes, and anything fancier
/// implements this trait directly.
pub trait PrecompileProvider<T: SvmTypesHost>: Send + Sync {
    /// Returns the handler for `program_id`, if this provider enables one.
    fn get(&self, program_id: &Address) -> Option<Arc<dyn Precompile<T>>>;

    /// Every program id this provider enables, in no particular order.
    fn program_ids(&self) -> Vec<Address>;
}

impl<T: SvmTypesHost> PrecompileProvider<T> for PrecompileSet<T> {
    fn get(&self, program_id: &Address) -> Option<Arc<dyn Precompile<T>>> {
        self.entries.get(program_id).cloned()
    }

    fn program_ids(&self) -> Vec<Address> {
        self.entries.keys().cloned().collect()
    }
}

/// The empty provider: no program id dispatches as a precompile.
///
/// This is the VM default when the `precompiles` feature is off, and the
/// explicit opt-out when it is on.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoPrecompiles;

impl<T: SvmTypesHost> PrecompileProvider<T> for NoPrecompiles {
    fn get(&self, _program_id: &Address) -> Option<Arc<dyn Precompile<T>>> {
        None
    }

    fn program_ids(&self) -> Vec<Address> {
        Vec::new()
    }
}

/// The agave-backed default set: today's precompile behaviour as a provider.
///
/// Built once from agave's precompile table. Each entry keeps its optional
/// feature gate, enforced at lookup through [`Precompile::is_enabled`], so a
/// gated native behaves exactly as it did under agave's `get_precompile`
/// feature check. Call [`into_set`](NativePrecompiles::into_set) to get an
/// editable set for adding or shadowing entries.
#[cfg(feature = "precompiles")]
pub struct NativePrecompiles<T: SvmTypesHost> {
    set: PrecompileSet<T>,
}

#[cfg(feature = "precompiles")]
impl<T: SvmTypesHost> NativePrecompiles<T> {
    /// Builds the default set from agave's precompile table.
    pub fn new() -> Self {
        let mut set = PrecompileSet::new();
        for precompile in agave_precompiles::get_precompiles() {
            set.insert(Arc::new(NativePrecompile {
                program_id: precompile.program_id,
                feature: precompile.feature,
                verify_fn: precompile.verify_fn,
            }));
        }
        Self { set }
    }

    /// Unpacks the default entries into an editable set.
    ///
    /// Insert under an existing program id to shadow that native.
    pub fn into_set(self) -> PrecompileSet<T> {
        self.set
    }
}

#[cfg(feature = "precompiles")]
impl<T: SvmTypesHost> Clone for NativePrecompiles<T> {
    fn clone(&self) -> Self {
        Self { set: self.set.clone() }
    }
}

#[cfg(feature = "precompiles")]
impl<T: SvmTypesHost> Default for NativePrecompiles<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "precompiles")]
impl<T: SvmTypesHost> fmt::Debug for NativePrecompiles<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativePrecompiles").field("program_ids", &self.set.program_ids()).finish()
    }
}

#[cfg(feature = "precompiles")]
impl<T: SvmTypesHost> PrecompileProvider<T> for NativePrecompiles<T> {
    fn get(&self, program_id: &Address) -> Option<Arc<dyn Precompile<T>>> {
        self.set.entries.get(program_id).cloned()
    }

    fn program_ids(&self) -> Vec<Address> {
        self.set.program_ids()
    }
}

/// One agave table row as a [`Precompile`]: program id, optional feature
/// gate, and the table's verify function.
#[cfg(feature = "precompiles")]
#[derive(Clone, Copy, Debug)]
struct NativePrecompile {
    program_id: Address,
    feature: Option<Address>,
    verify_fn: agave_precompiles::Verify,
}

#[cfg(feature = "precompiles")]
impl NativePrecompile {
    /// Whether this entry is live under `feature_set`.
    fn enabled(&self, feature_set: &FeatureSet) -> bool {
        // Dynamic id: each precompile carries its own optional feature, which
        // cannot be enumerated into the fixed `TxFeatureFlags` set. Checked
        // at lookup, mirroring agave's `get_precompile` feature check.
        self.feature.is_none_or(|feature| feature_set.is_active(&feature))
    }
}

#[cfg(feature = "precompiles")]
impl<T: SvmTypesHost> Precompile<T> for NativePrecompile {
    fn program_id(&self) -> Address {
        self.program_id
    }

    fn is_enabled(&self, feature_set: &FeatureSet) -> bool {
        self.enabled(feature_set)
    }

    fn verify(
        &self,
        data: &[u8],
        slices: &[&[u8]],
        feature_set: &FeatureSet,
    ) -> Result<(), PrecompileError> {
        // Dispatch filters through `is_enabled` first; the gate is re-checked
        // here so a direct `verify` cannot run a feature-disabled native.
        // A disabled entry reports the same `InvalidPublicKey` a lookup miss
        // produces.
        if self.enabled(feature_set) {
            (self.verify_fn)(data, slices, feature_set)
        } else {
            Err(PrecompileError::InvalidPublicKey)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BaseSvmTypes;

    #[derive(Debug)]
    struct StubPrecompile {
        program_id: Address,
    }

    impl<T: SvmTypesHost> Precompile<T> for StubPrecompile {
        fn program_id(&self) -> Address {
            self.program_id
        }

        fn verify(
            &self,
            _data: &[u8],
            _slices: &[&[u8]],
            _feature_set: &FeatureSet,
        ) -> Result<(), PrecompileError> {
            Ok(())
        }
    }

    fn stub() -> Arc<dyn Precompile<BaseSvmTypes>> {
        Arc::new(StubPrecompile { program_id: Address::new_unique() })
    }

    #[test]
    fn new_set_is_empty() {
        let set = PrecompileSet::<BaseSvmTypes>::new();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert!(set.program_ids().is_empty());
    }

    #[test]
    fn insert_get_contains_and_ids() {
        let first = stub();
        let second = stub();
        let mut set = PrecompileSet::<BaseSvmTypes>::new();
        assert!(set.insert(Arc::clone(&first)).is_none());
        assert!(set.insert(Arc::clone(&second)).is_none());

        assert_eq!(set.len(), 2);
        assert!(!set.is_empty());
        assert!(set.contains(&first.program_id()));
        assert!(set.contains(&second.program_id()));
        assert!(set.get(&first.program_id()).is_some());
        assert!(set.get(&Address::new_unique()).is_none());

        let mut ids = set.program_ids();
        ids.sort();
        let mut want = [first.program_id(), second.program_id()];
        want.sort();
        assert_eq!(ids, want);
    }

    #[test]
    fn insert_under_same_id_shadows_previous_entry() {
        let program_id = Address::new_unique();
        let first: Arc<dyn Precompile<BaseSvmTypes>> = Arc::new(StubPrecompile { program_id });
        let second: Arc<dyn Precompile<BaseSvmTypes>> = Arc::new(StubPrecompile { program_id });
        let mut set = PrecompileSet::<BaseSvmTypes>::new();
        assert!(set.insert(Arc::clone(&first)).is_none());

        let shadowed = set.insert(Arc::clone(&second));
        assert!(shadowed.is_some());
        assert!(Arc::ptr_eq(&shadowed.unwrap(), &first));
        assert_eq!(set.len(), 1);
        assert!(Arc::ptr_eq(&set.get(&program_id).unwrap(), &second));
    }

    #[test]
    fn clone_shares_handlers() {
        let entry = stub();
        let mut set = PrecompileSet::<BaseSvmTypes>::new();
        set.insert(Arc::clone(&entry));
        let cloned = set.clone();

        assert!(Arc::ptr_eq(&set.get(&entry.program_id()).unwrap(), &entry));
        assert!(Arc::ptr_eq(&cloned.get(&entry.program_id()).unwrap(), &entry));
    }

    #[test]
    fn no_precompiles_enables_nothing() {
        let provider = NoPrecompiles;
        let typed: &dyn PrecompileProvider<BaseSvmTypes> = &provider;
        assert!(typed.get(&Address::new_unique()).is_none());
        assert!(typed.program_ids().is_empty());
    }

    #[test]
    fn set_is_its_own_provider() {
        let entry = stub();
        let mut set = PrecompileSet::<BaseSvmTypes>::new();
        set.insert(Arc::clone(&entry));
        let provider: &dyn PrecompileProvider<BaseSvmTypes> = &set;

        assert!(provider.get(&entry.program_id()).is_some());
        assert_eq!(provider.program_ids(), [entry.program_id()]);
    }

    #[cfg(feature = "precompiles")]
    #[test]
    fn native_set_resolves_every_table_id() {
        let native = NativePrecompiles::<BaseSvmTypes>::new();
        for precompile in agave_precompiles::get_precompiles() {
            let handler = native.get(&precompile.program_id);
            assert!(handler.is_some(), "missing handler for {}", precompile.program_id);
            assert!(Arc::ptr_eq(
                &handler.unwrap(),
                &PrecompileProvider::<BaseSvmTypes>::get(&native, &precompile.program_id).unwrap()
            ));
        }
        assert!(!native.program_ids().is_empty());
    }
}
