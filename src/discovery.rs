//! This module contains the generated data structures from the [service protocol manifest schema](https://github.com/restatedev/service-protocol/blob/main/endpoint_manifest_schema.json).

mod generated {
    #![allow(clippy::clone_on_copy)]
    #![allow(clippy::to_string_trait_impl)]

    include!(concat!(env!("OUT_DIR"), "/endpoint_manifest.rs"));
}

pub use generated::*;
