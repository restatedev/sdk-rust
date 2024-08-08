mod generated {
    #![allow(clippy::clone_on_copy)]
    #![allow(clippy::to_string_trait_impl)]

    include!(concat!(env!("OUT_DIR"), "/endpoint_manifest.rs"));
}

pub use generated::*;
