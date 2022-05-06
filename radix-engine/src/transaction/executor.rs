use scrypto::crypto::hash;
use scrypto::engine::types::*;
use scrypto::resource::*;
use scrypto::rust::vec::Vec;
use scrypto::rust::vec;
use scrypto::rust::string::ToString;
use scrypto::{abi, rule, access_rule_node};

use crate::engine::*;
use crate::errors::*;
use crate::ledger::*;
use crate::model::*;
use crate::transaction::*;

/// An executor that runs transactions.
pub struct TransactionExecutor<'l, L: ReadableSubstateStore + WriteableSubstateStore> {
    substate_store: &'l mut L,
    trace: bool,
}

impl<'l, L: ReadableSubstateStore + WriteableSubstateStore> NonceProvider for TransactionExecutor<'l, L> {
    fn get_nonce<PKS: AsRef<[EcdsaPublicKey]>>(&self, _intended_signers: PKS) -> u64 {
        self.substate_store.get_nonce()
    }
}

impl<'l, L: ReadableSubstateStore + WriteableSubstateStore> AbiProvider for TransactionExecutor<'l, L> {
    fn export_abi(
        &self,
        package_address: PackageAddress,
        blueprint_name: &str,
    ) -> Result<abi::Blueprint, RuntimeError> {
        let package: Package = self
            .substate_store
            .get_decoded_substate(&package_address)
            .map(|(package, _)| package)
            .ok_or(RuntimeError::PackageNotFound(package_address))?;

        BasicAbiProvider::new(self.trace)
            .with_package(&package_address, package)
            .export_abi(package_address, blueprint_name)
    }

    fn export_abi_by_component(
        &self,
        component_address: ComponentAddress,
    ) -> Result<abi::Blueprint, RuntimeError> {
        let component: Component = self
            .substate_store
            .get_decoded_substate(&component_address)
            .map(|(component, _)| component)
            .ok_or(RuntimeError::ComponentNotFound(component_address))?;
        let package: Package = self
            .substate_store
            .get_decoded_substate(&component.package_address())
            .map(|(package, _)| package)
            .unwrap();
        BasicAbiProvider::new(self.trace)
            .with_package(&component.package_address(), package)
            .export_abi(component.package_address(), component.blueprint_name())
    }
}

impl<'l, L: ReadableSubstateStore + WriteableSubstateStore> TransactionExecutor<'l, L> {
    pub fn new(substate_store: &'l mut L, trace: bool) -> Self {
        Self {
            substate_store,
            trace,
        }
    }

    /// Returns an immutable reference to the ledger.
    pub fn substate_store(&self) -> &L {
        self.substate_store
    }

    /// Returns a mutable reference to the ledger.
    pub fn substate_store_mut(&mut self) -> &mut L {
        self.substate_store
    }

    /// Generates a new key pair.
    pub fn new_key_pair(&mut self) -> (EcdsaPublicKey, EcdsaPrivateKey) {
        let nonce = self.substate_store.get_nonce();
        self.substate_store.increase_nonce();
        let private_key = EcdsaPrivateKey::from_bytes(
            hash(nonce.to_le_bytes()).as_ref(),
        )
        .unwrap();
        let public_key = private_key.public_key();
        (public_key, private_key)
    }

    /// Creates an account with 1,000,000 XRD in balance.
    pub fn new_account_with_auth_rule(&mut self, withdraw_auth: &AccessRule) -> ComponentAddress {
        let receipt = self
            .validate_and_execute(
                &TransactionBuilder::new()
                    .call_method(SYSTEM_COMPONENT, "free_xrd", vec![])
                    .take_from_worktop(RADIX_TOKEN, |builder, bucket_id| {
                        builder.new_account_with_resource(withdraw_auth, bucket_id)
                    })
                    .build(self.get_nonce([]))
                    .sign([]),
            )
            .unwrap();

        receipt.result.expect("Should be okay");
        receipt.new_component_addresses[0]
    }

    /// Creates a new key and an account which can be accessed using the key.
    pub fn new_account(&mut self) -> (EcdsaPublicKey, EcdsaPrivateKey, ComponentAddress) {
        let (public_key, private_key) = self.new_key_pair();
        let id = NonFungibleId::from_bytes(public_key.to_vec());
        let auth_address = NonFungibleAddress::new(ECDSA_TOKEN, id);
        let withdraw_auth = rule!(require(auth_address));
        let account = self.new_account_with_auth_rule(&withdraw_auth);
        (public_key, private_key, account)
    }

    /// Publishes a package.
    pub fn publish_package<T: AsRef<[u8]>>(
        &mut self,
        code: T,
    ) -> Result<PackageAddress, RuntimeError> {
        let receipt = self
            .validate_and_execute(
                &TransactionBuilder::new()
                    .publish_package(code.as_ref())
                    .build(self.get_nonce([]))
                    .sign([]),
            )
            .unwrap();

        if receipt.result.is_ok() {
            Ok(receipt.new_package_addresses[0])
        } else {
            Err(receipt.result.err().unwrap())
        }
    }

    pub fn validate_and_execute(
        &mut self,
        signed: &SignedTransaction,
    ) -> Result<Receipt, TransactionValidationError> {
        let validated = signed.validate()?;
        let receipt = self.execute(validated);
        Ok(receipt)
    }

    pub fn execute(&mut self, validated: ValidatedTransaction) -> Receipt {
        #[cfg(not(feature = "alloc"))]
        let now = std::time::Instant::now();

        let mut track = Track::new(
            self.substate_store,
            validated.raw_hash.clone(),
            validated.signers.clone(),
        );
        let mut proc = track.start_process(self.trace);

        let mut txn_process = TransactionProcess::new(validated.clone());
        let txn_snode = SNodeState::Transaction(&mut txn_process);
        let error = match proc.run(None, txn_snode, "execute".to_string(), vec![]) {
            Ok(_) => None,
            Err(e) => Some(e),
        };
        let outputs = txn_process.outputs().to_vec();

        let track_receipt = track.to_receipt();
        // commit state updates
        let commit_receipt = if error.is_none() {
            if !track_receipt.borrowed.is_empty() {
                panic!("There should be nothing borrowed by end of transaction.");
            }
            let commit_receipt = track_receipt.substates.commit(self.substate_store);
            self.substate_store.increase_nonce();
            Some(commit_receipt)
        } else {
            None
        };

        let mut new_component_addresses = Vec::new();
        let mut new_resource_addresses = Vec::new();
        let mut new_package_addresses = Vec::new();
        for address in track_receipt.new_addresses {
            match address {
                Address::Component(component_address) => new_component_addresses.push(component_address),
                Address::Resource(resource_address) => new_resource_addresses.push(resource_address),
                Address::Package(package_address) => new_package_addresses.push(package_address),
            }
        }


        #[cfg(feature = "alloc")]
        let execution_time = None;
        #[cfg(not(feature = "alloc"))]
        let execution_time = Some(now.elapsed().as_millis());

        Receipt {
            commit_receipt,
            validated_transaction: validated.clone(),
            result: match error {
                Some(error) => Err(error),
                None => Ok(()),
            },
            outputs,
            logs: track_receipt.logs,
            new_package_addresses,
            new_component_addresses,
            new_resource_addresses,
            execution_time,
        }
    }
}
