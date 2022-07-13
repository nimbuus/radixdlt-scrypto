/// A unique identifier used in the addressing of Resource Addresses.
pub const RESOURCE_ADDRESS_ENTITY_ID: u8 = 0x00;

/// A unique identifier used in the addressing of Package Addresses.
pub const PACKAGE_ADDRESS_ENTITY_ID: u8 = 0x01;

/// A unique identifier used in the addressing of Generic Component Addresses.
pub const COMPONENT_ADDRESS_ENTITY_ID: u8 = 0x02;

/// A unique identifier used in the addressing of Account Component Addresses.
pub const ACCOUNT_COMPONENT_ADDRESS_ENTITY_ID: u8 = 0x03;

/// A unique identifier used in the addressing of System Component Addresses.
pub const SYSTEM_COMPONENT_ADDRESS_ENTITY_ID: u8 = 0x04;

/// An enum which represents the different addressable entities.
#[derive(PartialEq, Eq)]
pub enum EntityType {
    Resource,
    Package,
    Component,
    AccountComponent,
    SystemComponent,
}

impl EntityType {
    pub fn id(&self) -> u8 {
        match self {
            Self::Resource => RESOURCE_ADDRESS_ENTITY_ID,
            Self::Package => PACKAGE_ADDRESS_ENTITY_ID,
            Self::Component => COMPONENT_ADDRESS_ENTITY_ID,
            Self::AccountComponent => ACCOUNT_COMPONENT_ADDRESS_ENTITY_ID,
            Self::SystemComponent => SYSTEM_COMPONENT_ADDRESS_ENTITY_ID,
        }
    }
}

impl TryFrom<u8> for EntityType {
    type Error = EntityTypeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            RESOURCE_ADDRESS_ENTITY_ID => Ok(Self::Resource),
            PACKAGE_ADDRESS_ENTITY_ID => Ok(Self::Package),
            COMPONENT_ADDRESS_ENTITY_ID => Ok(Self::Component),
            ACCOUNT_COMPONENT_ADDRESS_ENTITY_ID => Ok(Self::AccountComponent),
            SYSTEM_COMPONENT_ADDRESS_ENTITY_ID => Ok(Self::SystemComponent),
            _ => Err(EntityTypeError::InvalidEntityTypeId(value)),
        }
    }
}

pub enum EntityTypeError {
    InvalidEntityTypeId(u8),
}

/// Represents the allowed list of entity types that packages can have.
pub const ALLOWED_PACKAGE_ENTITY_TYPES: [EntityType; 1] = [EntityType::Package];
/// Represents the allowed list of entity types that resources can have.
pub const ALLOWED_RESOURCE_ENTITY_TYPES: [EntityType; 1] = [EntityType::Resource];
/// Represents the allowed list of entity types that components can have.
pub const ALLOWED_COMPONENT_ENTITY_TYPES: [EntityType; 3] = [
    EntityType::Component,
    EntityType::AccountComponent,
    EntityType::SystemComponent,
];