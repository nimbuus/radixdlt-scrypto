use sbor::*;
use scrypto::engine::types::*;
use scrypto::rust::vec::Vec;

use crate::errors::RuntimeError;
use crate::errors::RuntimeError::NotAuthorized;
use crate::model::Proof;

#[derive(Debug, Clone, PartialEq, Eq, Hash, TypeId, Encode, Decode)]
pub enum HardResourceOrNonFungible {
    NonFungible(NonFungibleAddress),
    Resource(ResourceAddress),
    SoftResourceNotFound,
}

impl HardResourceOrNonFungible {
    pub fn proof_matches(&self, proof: &Proof) -> bool {
        match self {
            HardResourceOrNonFungible::NonFungible(non_fungible_address) => {
                let proof_resource_address = proof.resource_address();
                proof_resource_address == non_fungible_address.resource_address()
                    && match proof.total_ids() {
                        Ok(ids) => ids.contains(&non_fungible_address.non_fungible_id()),
                        Err(_) => false,
                    }
            }
            HardResourceOrNonFungible::Resource(resource_address) => {
                let proof_resource_address = proof.resource_address();
                proof_resource_address == *resource_address
            }
            HardResourceOrNonFungible::SoftResourceNotFound => false,
        }
    }

    pub fn check_has_amount(&self, amount: Decimal, proofs_vector: &[&[Proof]]) -> bool {
        for proofs in proofs_vector {
            // FIXME: Need to check the composite max amount rather than just each proof individually
            if proofs
                .iter()
                .any(|p| self.proof_matches(p) && p.total_amount() >= amount)
            {
                return true;
            }
        }

        false
    }

    pub fn check(&self, proofs_vector: &[&[Proof]]) -> bool {
        for proofs in proofs_vector {
            if proofs.iter().any(|p| self.proof_matches(p)) {
                return true;
            }
        }

        false
    }
}

impl From<NonFungibleAddress> for HardResourceOrNonFungible {
    fn from(non_fungible_address: NonFungibleAddress) -> Self {
        HardResourceOrNonFungible::NonFungible(non_fungible_address)
    }
}

impl From<ResourceAddress> for HardResourceOrNonFungible {
    fn from(resource_address: ResourceAddress) -> Self {
        HardResourceOrNonFungible::Resource(resource_address)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, TypeId, Encode, Decode)]
pub enum HardProofRuleResourceList {
    List(Vec<HardResourceOrNonFungible>),
    SoftResourceListNotFound,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, TypeId, Encode, Decode)]
pub enum HardProofRule {
    This(HardResourceOrNonFungible),
    SomeOfResource(Decimal, HardResourceOrNonFungible),
    AllOf(HardProofRuleResourceList),
    AnyOf(HardProofRuleResourceList),
    CountOf(u8, HardProofRuleResourceList),
}

impl HardProofRule {
    pub fn check(&self, proofs_vector: &[&[Proof]]) -> Result<(), RuntimeError> {
        match self {
            HardProofRule::This(resource) => {
                if resource.check(proofs_vector) {
                    Ok(())
                } else {
                    Err(NotAuthorized)
                }
            }
            HardProofRule::SomeOfResource(amount, resource) => {
                if resource.check_has_amount(*amount, proofs_vector) {
                    Ok(())
                } else {
                    Err(NotAuthorized)
                }
            }
            HardProofRule::AllOf(resource_list) => match resource_list {
                HardProofRuleResourceList::SoftResourceListNotFound => Err(NotAuthorized),
                HardProofRuleResourceList::List(resources) => {
                    for resource in resources {
                        if !resource.check(proofs_vector) {
                            return Err(NotAuthorized);
                        }
                    }

                    Ok(())
                }
            },
            HardProofRule::AnyOf(resource_list) => match resource_list {
                HardProofRuleResourceList::SoftResourceListNotFound => Err(NotAuthorized),
                HardProofRuleResourceList::List(resources) => {
                    for resource in resources {
                        if resource.check(proofs_vector) {
                            return Ok(());
                        }
                    }

                    Err(NotAuthorized)
                }
            },
            HardProofRule::CountOf(count, resource_list) => match resource_list {
                HardProofRuleResourceList::SoftResourceListNotFound => Err(NotAuthorized),
                HardProofRuleResourceList::List(resources) => {
                    let mut left = count.clone();
                    for resource in resources {
                        if resource.check(proofs_vector) {
                            left -= 1;
                            if left == 0 {
                                return Ok(());
                            }
                        }
                    }
                    Err(NotAuthorized)
                }
            },
        }
    }
}

/// Snode which verifies authorization of a method call
#[derive(Debug, Clone, PartialEq, Eq, Hash, TypeId, Encode, Decode)]
pub enum MethodAuthorization {
    Protected(HardProofRule),
    Public,
    Private,
    Unsupported,
}

impl MethodAuthorization {
    pub fn check(&self, proofs_vector: &[&[Proof]]) -> Result<(), RuntimeError> {
        match self {
            MethodAuthorization::Protected(rule) => rule.check(proofs_vector),
            MethodAuthorization::Public => Ok(()),
            MethodAuthorization::Private => Err(RuntimeError::NotAuthorized),
            MethodAuthorization::Unsupported => Err(RuntimeError::UnsupportedMethod),
        }
    }
}
