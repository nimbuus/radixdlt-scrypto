use scrypto::prelude::*;

blueprint! {
    struct PackageTest;

    impl PackageTest {
        pub fn publish() -> PackageAddress {
            let package = Package {
                code: include_bytes!("../../../../assets/system.wasm").to_vec(),
                blueprint_abis: HashMap::new(),
            };
            component_system().publish_package(package)
        }
    }
}
