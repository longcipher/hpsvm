//! Type bridge between the workspace's newer Solana interface types and the
//! older copies the agave 4.5 runtime crates are compiled against.
//!
//! The root `Cargo.toml` pins `solana-message` 6.0 / `solana-transaction` 6.0 /
//! `solana-svm-transaction` 8.0, while agave 4.5 (`solana-program-runtime`,
//! `solana-fee`, `solana-compute-budget-instruction`) is built against
//! `solana-message` 5.1 / `solana-transaction` 5.1 / `solana-svm-transaction`
//! 6.0. Both families therefore coexist in the dependency graph. They are
//! structurally identical — the only difference is the defining crate (and
//! `solana-pubkey` is a re-export of `solana-address`, so `Pubkey` *is*
//! `Address`) — so this module re-wraps borrowed data under the older trait
//! definitions instead of converting whole transactions or downgrading the
//! pins.

use solana_address::Address;
use solana_hash::Hash;
use solana_message::{SanitizedMessage, compiled_instruction::CompiledInstruction};
use solana_message_agave::{AccountKeys, v0::LoadedAddresses as AgaveLoadedAddresses};
use solana_svm_transaction_agave::{
    instruction::SVMInstruction as AgaveSVMInstruction,
    message_address_table_lookup::SVMMessageAddressTableLookup,
    svm_message::{SVMMessage, SVMStaticMessage},
};
use solana_transaction_agave::versioned::TransactionVersion;

/// Borrowed view of a sanitized message that implements the agave-4.5
/// (`solana-svm-transaction` 6.0.0) message traits.
///
/// Every method delegates to the inherent API of the workspace's
/// `solana-message` 6.0 [`SanitizedMessage`]; instruction and lookup data is
/// re-wrapped in the agave-side shapes, which are field-identical.
pub(crate) struct AgaveMessage<'a> {
    message: &'a SanitizedMessage,
    /// Resolved address-lookup-table addresses in the agave-side shape.
    ///
    /// The agave `AccountKeys` borrows an owned `LoadedAddresses`, so the
    /// converted copy lives here for the lifetime of the view. Only populated
    /// for V0 messages; legacy and V1 messages resolve their keys statically.
    loaded_addresses: Option<AgaveLoadedAddresses>,
}

impl<'a> AgaveMessage<'a> {
    pub(crate) fn new(message: &'a SanitizedMessage) -> Self {
        // Same match as `SanitizedMessage::loaded_lookup_table_addresses`,
        // which is private to `solana-message`.
        let loaded_addresses = match message {
            SanitizedMessage::V0(loaded) => Some(AgaveLoadedAddresses {
                writable: loaded.loaded_addresses.writable.clone(),
                readonly: loaded.loaded_addresses.readonly.clone(),
            }),
            _ => None,
        };
        Self { message, loaded_addresses }
    }
}

impl core::fmt::Debug for AgaveMessage<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AgaveMessage").field("message", &self.message).finish()
    }
}

/// Re-wrap a [`CompiledInstruction`] from the workspace's `solana-message` 6.0
/// into the agave-4.5 6.0.0 [`AgaveSVMInstruction`] shape. The two structs are
/// field-identical; only the defining crate differs.
#[inline]
pub(crate) fn to_agave_instruction(instruction: &CompiledInstruction) -> AgaveSVMInstruction<'_> {
    AgaveSVMInstruction {
        program_id_index: instruction.program_id_index,
        accounts: &instruction.accounts,
        data: &instruction.data,
    }
}

impl SVMStaticMessage for AgaveMessage<'_> {
    fn version(&self) -> TransactionVersion {
        match self.message {
            SanitizedMessage::Legacy(_) => TransactionVersion::LEGACY,
            SanitizedMessage::V0(_) => TransactionVersion::Number(0),
            SanitizedMessage::V1(_) => TransactionVersion::Number(1),
        }
    }

    fn num_transaction_signatures(&self) -> u64 {
        u64::from(self.message.header().num_required_signatures)
    }

    fn num_write_locks(&self) -> u64 {
        self.message.num_write_locks()
    }

    fn num_readonly_signed_static_accounts(&self) -> u8 {
        self.message.header().num_readonly_signed_accounts
    }

    fn num_readonly_unsigned_static_accounts(&self) -> u8 {
        self.message.header().num_readonly_unsigned_accounts
    }

    fn recent_blockhash(&self) -> &Hash {
        self.message.recent_blockhash()
    }

    fn num_instructions(&self) -> usize {
        self.message.instructions().len()
    }

    fn instructions_iter(&self) -> impl Iterator<Item = AgaveSVMInstruction<'_>> {
        self.message.instructions().iter().map(to_agave_instruction)
    }

    fn program_instructions_iter(
        &self,
    ) -> impl Iterator<Item = (&Address, AgaveSVMInstruction<'_>)> + Clone {
        self.message
            .program_instructions_iter()
            .map(|(program_id, instruction)| (program_id, to_agave_instruction(instruction)))
    }

    fn static_account_keys(&self) -> &[Address] {
        self.message.static_account_keys()
    }

    fn fee_payer(&self) -> &Address {
        self.message.fee_payer()
    }

    fn num_lookup_tables(&self) -> usize {
        self.message.message_address_table_lookups().len()
    }

    fn message_address_table_lookups(
        &self,
    ) -> impl Iterator<Item = SVMMessageAddressTableLookup<'_>> {
        self.message.message_address_table_lookups().iter().map(|lookup| {
            SVMMessageAddressTableLookup {
                account_key: &lookup.account_key,
                writable_indexes: &lookup.writable_indexes,
                readonly_indexes: &lookup.readonly_indexes,
            }
        })
    }

    fn is_signer(&self, index: usize) -> bool {
        self.message.is_signer(index)
    }

    fn is_invoked(&self, key_index: usize) -> bool {
        self.message.is_invoked(key_index)
    }
}

impl SVMMessage for AgaveMessage<'_> {
    fn account_keys(&self) -> AccountKeys<'_> {
        AccountKeys::new(self.message.static_account_keys(), self.loaded_addresses.as_ref())
    }

    fn is_writable(&self, index: usize) -> bool {
        self.message.is_writable(index)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use solana_hash::Hash;
    use solana_message::{
        AddressLoader, MessageHeader, SanitizedVersionedMessage, VersionedMessage, legacy,
        v0::{LoadedAddresses, Message as V0Message, MessageAddressTableLookup},
    };
    use solana_sdk_ids::system_program;
    // The workspace's own (8.0.0) copy of the trait, used to cross-check the
    // agave-side delegation against the implementation `vm.rs` calls into.
    use solana_svm_transaction::svm_message::SVMStaticMessage as WorkspaceSVMStaticMessage;
    use solana_transaction_error::AddressLoaderError;

    use super::*;

    /// Serialized value of `SystemInstruction::AdvanceNonceAccount`.
    const ADVANCE_NONCE_DATA: [u8; 4] = 4u32.to_le_bytes();

    fn address(tag: u8) -> Address {
        Address::new_from_array([tag; 32])
    }

    fn blockhash() -> Hash {
        Hash::new_from_array([7; 32])
    }

    /// Resolves each lookup by its table account key. The header counts the
    /// tests assert (readonly signed/unsigned, write locks, lookup tables) are
    /// deliberately not 0 or 1, so mutations that replace them with a constant
    /// cannot pass as equivalent.
    #[derive(Clone)]
    struct FixedLoader {
        tables: Vec<(Address, LoadedAddresses)>,
    }

    impl AddressLoader for FixedLoader {
        fn load_addresses(
            self,
            lookups: &[MessageAddressTableLookup],
        ) -> Result<LoadedAddresses, AddressLoaderError> {
            lookups
                .iter()
                .map(|lookup| {
                    self.tables
                        .iter()
                        .find(|(table_key, _)| *table_key == lookup.account_key)
                        .map(|(_, loaded)| loaded.clone())
                        .ok_or(AddressLoaderError::LookupTableAccountNotFound)
                })
                .collect()
        }
    }

    fn no_tables() -> FixedLoader {
        FixedLoader { tables: Vec::new() }
    }

    fn sanitize(message: VersionedMessage, loader: FixedLoader) -> SanitizedMessage {
        SanitizedMessage::try_new(
            SanitizedVersionedMessage::try_new(message).expect("message should sanitize"),
            loader,
            &HashSet::new(),
        )
        .expect("message should sanitize into a sanitized message")
    }

    /// `[payer(signer, writable), signer(signer, readonly-signed),
    /// program(unsigned, writable), data(unsigned, readonly),
    /// data(unsigned, readonly)]` with two instructions calling `program`.
    fn legacy_message() -> (legacy::Message, Vec<Address>, Address) {
        let keys: Vec<Address> = (1..=5).map(address).collect();
        let program = keys[2];
        let message = legacy::Message {
            header: MessageHeader {
                num_required_signatures: 2,
                num_readonly_signed_accounts: 1,
                num_readonly_unsigned_accounts: 2,
            },
            account_keys: keys.clone(),
            recent_blockhash: blockhash(),
            instructions: vec![
                CompiledInstruction::new_from_raw_parts(2, vec![1, 2, 3], vec![0, 3]),
                CompiledInstruction::new_from_raw_parts(2, vec![4], vec![1, 4]),
            ],
        };
        (message, keys, program)
    }

    #[test]
    fn to_agave_instruction_copies_every_field() {
        let compiled = CompiledInstruction {
            program_id_index: 9,
            accounts: vec![0, 3, 7],
            data: vec![42, 43],
        };
        let instruction = to_agave_instruction(&compiled);
        assert_eq!(instruction.program_id_index, 9);
        assert_eq!(instruction.accounts, &[0, 3, 7]);
        assert_eq!(instruction.data, &[42, 43]);
    }

    #[test]
    fn legacy_message_delegates_static_fields() {
        let (message, keys, program) = legacy_message();
        let sanitized = sanitize(VersionedMessage::Legacy(message), no_tables());
        let bridge = AgaveMessage::new(&sanitized);

        assert_eq!(bridge.version(), TransactionVersion::LEGACY);
        assert_eq!(bridge.num_transaction_signatures(), 2);
        assert_eq!(bridge.num_instructions(), 2);
        assert_eq!(bridge.static_account_keys(), keys);
        assert_eq!(bridge.fee_payer(), &keys[0]);
        assert_eq!(bridge.recent_blockhash(), &blockhash());
        assert_eq!(bridge.num_write_locks(), 2);
        assert_eq!(bridge.num_readonly_signed_static_accounts(), 1);
        assert_eq!(bridge.num_readonly_unsigned_static_accounts(), 2);
        assert_eq!(bridge.num_lookup_tables(), 0);
        assert_eq!(bridge.message_address_table_lookups().count(), 0);

        let account_keys = bridge.account_keys();
        assert_eq!(account_keys.len(), 5);
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(account_keys.get(index), Some(key));
        }

        assert!(bridge.is_signer(0));
        assert!(bridge.is_signer(1));
        assert!(!bridge.is_signer(2));
        assert!(!bridge.is_signer(3));
        assert!(!bridge.is_signer(4));
        assert!(bridge.is_writable(0));
        assert!(!bridge.is_writable(1));
        // Index 2 is the program id: sanitization demotes its write lock, so the
        // demotion-aware view is readonly even though `num_write_locks` (which
        // counts *requested* locks) still includes it.
        assert!(!bridge.is_writable(2));
        assert!(!bridge.is_writable(3));
        assert!(!bridge.is_writable(4));
        assert!(!bridge.is_invoked(0));
        assert!(!bridge.is_invoked(1));
        assert!(bridge.is_invoked(2));
        assert!(!bridge.is_invoked(3));
        assert!(!bridge.is_invoked(4));
        assert_eq!(program, keys[2]);
    }

    #[test]
    fn legacy_message_delegates_instructions() {
        let (message, _keys, program) = legacy_message();
        let sanitized = sanitize(VersionedMessage::Legacy(message), no_tables());
        let bridge = AgaveMessage::new(&sanitized);

        let instructions: Vec<_> = bridge.instructions_iter().collect();
        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[0].program_id_index, 2);
        assert_eq!(instructions[0].accounts, &[0, 3]);
        assert_eq!(instructions[0].data, &[1, 2, 3]);
        assert_eq!(instructions[1].accounts, &[1, 4]);
        assert_eq!(instructions[1].data, &[4]);

        let program_instructions: Vec<_> = bridge.program_instructions_iter().collect();
        assert_eq!(program_instructions.len(), 2);
        assert_eq!(program_instructions[0].0, &program);
        assert_eq!(program_instructions[0].1.data, &[1, 2, 3]);
        assert_eq!(program_instructions[1].0, &program);
        assert_eq!(program_instructions[1].1.data, &[4]);
    }

    /// The fee path is the main consumer of the signature-count delegation, so
    /// drive it through the real agave API rather than the trait methods alone.
    #[test]
    fn fee_matches_signature_count_through_agave_api() {
        let (message, _keys, _program) = legacy_message();
        let sanitized = sanitize(VersionedMessage::Legacy(message), no_tables());
        let bridge = AgaveMessage::new(&sanitized);

        let lamports_per_signature = 5_000;
        let fee = solana_fee::calculate_fee(
            &bridge,
            lamports_per_signature,
            0,
            solana_fee::FeeFeatures {},
        );
        assert_eq!(fee, 2 * lamports_per_signature);
    }

    #[test]
    fn v0_message_resolves_loaded_addresses() {
        // `[payer(signer, writable), signer(signer, readonly-signed),
        // signer(signer, readonly-signed), program(unsigned, writable)]`, two
        // address lookup tables contributing one writable and one readonly
        // address each.
        let keys: Vec<Address> = (1..=4).map(address).collect();
        let program = keys[3];
        let table_one = address(11);
        let table_two = address(12);
        let loaded_writable_one = address(13);
        let loaded_writable_two = address(14);
        let loaded_readonly_one = address(15);
        let loaded_readonly_two = address(16);
        let message = V0Message {
            header: MessageHeader {
                num_required_signatures: 3,
                num_readonly_signed_accounts: 2,
                num_readonly_unsigned_accounts: 0,
            },
            account_keys: keys.clone(),
            recent_blockhash: blockhash(),
            instructions: vec![
                CompiledInstruction::new_from_raw_parts(3, vec![9], vec![0, 4, 6]),
                CompiledInstruction::new_from_raw_parts(3, Vec::new(), vec![1, 5, 7]),
            ],
            address_table_lookups: vec![
                MessageAddressTableLookup {
                    account_key: table_one,
                    writable_indexes: vec![0],
                    readonly_indexes: vec![1],
                },
                MessageAddressTableLookup {
                    account_key: table_two,
                    writable_indexes: vec![1],
                    readonly_indexes: vec![0],
                },
            ],
        };
        let sanitized = sanitize(
            VersionedMessage::V0(message),
            FixedLoader {
                tables: vec![
                    (
                        table_one,
                        LoadedAddresses {
                            writable: vec![loaded_writable_one],
                            readonly: vec![loaded_readonly_one],
                        },
                    ),
                    (
                        table_two,
                        LoadedAddresses {
                            writable: vec![loaded_writable_two],
                            readonly: vec![loaded_readonly_two],
                        },
                    ),
                ],
            },
        );
        let bridge = AgaveMessage::new(&sanitized);

        assert_eq!(bridge.version(), TransactionVersion::Number(0));
        assert_eq!(bridge.num_transaction_signatures(), 3);
        assert_eq!(bridge.num_instructions(), 2);
        assert_eq!(bridge.num_lookup_tables(), 2);
        assert_eq!(bridge.num_readonly_signed_static_accounts(), 2);
        assert_eq!(bridge.num_readonly_unsigned_static_accounts(), 0);
        assert_eq!(bridge.num_write_locks(), 4);
        assert_eq!(bridge.static_account_keys(), keys);

        let lookups: Vec<_> = bridge.message_address_table_lookups().collect();
        assert_eq!(lookups.len(), 2);
        assert_eq!(lookups[0].account_key, &table_one);
        assert_eq!(lookups[0].writable_indexes, &[0]);
        assert_eq!(lookups[0].readonly_indexes, &[1]);
        assert_eq!(lookups[1].account_key, &table_two);
        assert_eq!(lookups[1].writable_indexes, &[1]);
        assert_eq!(lookups[1].readonly_indexes, &[0]);

        // Static keys first, then writable loaded addresses, then readonly ones.
        let account_keys = bridge.account_keys();
        assert_eq!(account_keys.len(), 8);
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(account_keys.get(index), Some(key));
        }
        assert_eq!(account_keys.get(4), Some(&loaded_writable_one));
        assert_eq!(account_keys.get(5), Some(&loaded_writable_two));
        assert_eq!(account_keys.get(6), Some(&loaded_readonly_one));
        assert_eq!(account_keys.get(7), Some(&loaded_readonly_two));

        assert!(bridge.is_writable(0));
        assert!(!bridge.is_writable(1));
        assert!(!bridge.is_writable(2));
        // Index 3 is the program id: its write lock is demoted.
        assert!(!bridge.is_writable(3));
        assert!(bridge.is_writable(4));
        assert!(bridge.is_writable(5));
        assert!(!bridge.is_writable(6));
        assert!(!bridge.is_writable(7));
        assert!(bridge.is_signer(0));
        assert!(bridge.is_signer(1));
        assert!(bridge.is_signer(2));
        assert!(!bridge.is_signer(3));
        assert!(!bridge.is_signer(4));
        assert!(!bridge.is_invoked(0));
        assert!(bridge.is_invoked(3));
        assert_eq!(program, keys[3]);
    }

    /// Legacy `[payer(signer, writable), nonce(writable), system(readonly)]`
    /// whose first instruction advances the nonce.
    fn nonce_message(with_nonce_as_program: bool) -> (legacy::Message, Address, Address) {
        let payer = address(1);
        let nonce = address(2);
        let system = system_program::id();
        let mut instructions = vec![CompiledInstruction::new_from_raw_parts(
            2,
            ADVANCE_NONCE_DATA.to_vec(),
            vec![1, 0],
        )];
        if with_nonce_as_program {
            instructions.push(CompiledInstruction::new_from_raw_parts(1, Vec::new(), Vec::new()));
        }
        let message = legacy::Message {
            header: MessageHeader {
                num_required_signatures: 1,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 1,
            },
            account_keys: vec![payer, nonce, system],
            recent_blockhash: blockhash(),
            instructions,
        };
        (message, payer, nonce)
    }

    #[test]
    fn durable_nonce_is_detected_through_both_copies() {
        let (message, _payer, nonce) = nonce_message(false);
        let sanitized = sanitize(VersionedMessage::Legacy(message), no_tables());
        let bridge = AgaveMessage::new(&sanitized);

        // The agave-4.5 (6.0.0) default impl and the workspace's 8.0 impl must
        // agree on the nonce account, otherwise the two copies in the graph
        // would disagree about which transactions are nonce-based.
        assert_eq!(SVMMessage::get_durable_nonce(&bridge), Some(&nonce));
        assert_eq!(WorkspaceSVMStaticMessage::get_durable_nonce(&sanitized, false), Some(&nonce));
    }

    #[test]
    fn durable_nonce_as_program_id_is_demoted_under_agave_semantics() {
        let (message, _payer, nonce) = nonce_message(true);
        let sanitized = sanitize(VersionedMessage::Legacy(message), no_tables());
        let bridge = AgaveMessage::new(&sanitized);

        // The two copies disagree for this pathological message: the agave-4.5
        // impl this crate's nonce check uses is write-lock-demotion aware (a
        // nonce invoked as a program id loses its write lock, so it is not a
        // durable nonce), while the 8.0 impl ignores demotion and only rejects
        // the nonce under the SIMD-0602 flag. `check_message_for_nonce` routes
        // through the agave-side impl to keep the pre-upgrade behaviour.
        assert_eq!(SVMMessage::get_durable_nonce(&bridge), None);
        assert_eq!(WorkspaceSVMStaticMessage::get_durable_nonce(&sanitized, false), Some(&nonce));
        assert_eq!(WorkspaceSVMStaticMessage::get_durable_nonce(&sanitized, true), None);
    }
}
