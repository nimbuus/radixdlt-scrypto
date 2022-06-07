use sbor::rust::collections::BTreeSet;
use sbor::rust::string::String;
use sbor::rust::vec::Vec;
use sbor::*;
use scrypto::crypto::*;
use scrypto::engine::types::*;
use scrypto::values::*;

/// Represents an instruction that can be executed by Radix Engine.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, TypeId)]
pub enum ExecutableInstruction {
    TakeFromWorktop {
        resource_address: ResourceAddress,
    },
    TakeFromWorktopByAmount {
        amount: Decimal,
        resource_address: ResourceAddress,
    },
    TakeFromWorktopByIds {
        ids: BTreeSet<NonFungibleId>,
        resource_address: ResourceAddress,
    },
    ReturnToWorktop {
        bucket_id: BucketId,
    },
    AssertWorktopContains {
        resource_address: ResourceAddress,
    },
    AssertWorktopContainsByAmount {
        amount: Decimal,
        resource_address: ResourceAddress,
    },
    AssertWorktopContainsByIds {
        ids: BTreeSet<NonFungibleId>,
        resource_address: ResourceAddress,
    },
    PopFromAuthZone,
    PushToAuthZone {
        proof_id: ProofId,
    },
    ClearAuthZone,
    CreateProofFromAuthZone {
        resource_address: ResourceAddress,
    },
    CreateProofFromAuthZoneByAmount {
        amount: Decimal,
        resource_address: ResourceAddress,
    },
    CreateProofFromAuthZoneByIds {
        ids: BTreeSet<NonFungibleId>,
        resource_address: ResourceAddress,
    },
    CreateProofFromBucket {
        bucket_id: BucketId,
    },
    CloneProof {
        proof_id: ProofId,
    },
    DropProof {
        proof_id: ProofId,
    },
    CallFunction {
        package_address: PackageAddress,
        blueprint_name: String,
        call_data: ScryptoValue,
    },
    CallMethod {
        component_address: ComponentAddress,
        call_data: ScryptoValue,
    },
    CallMethodWithAllResources {
        component_address: ComponentAddress,
        method: String,
    },
    PublishPackage {
        package: Vec<u8>,
    },
}

/// A common trait for all transactions that can be executed by Radix Engine.
pub trait ExecutableTransaction {
    /// Returns the transaction hash, which must be globally unique.
    fn transaction_hash(&self) -> Hash;

    /// Returns the instructions to execute.
    fn instructions(&self) -> &[ExecutableInstruction];

    /// Returns the public key of signers.
    fn signer_public_keys(&self) -> &[EcdsaPublicKey];
}