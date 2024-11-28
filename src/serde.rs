//! # Serialization
//!
//! Restate sends data over the network for storing state, journaling actions, awakeables, etc.
//!
//! Therefore, all data structures need to have serializers and deserializers.
//!
//! You can use [Rust Serde](https://serde.rs/) for this.
//! Either [use derive](https://serde.rs/derive.html) or implement the [`Serialize` and `Deserialize` traits](https://serde.rs/custom-serialization.html).
//!

use bytes::Bytes;
use std::convert::Infallible;

const APPLICATION_JSON: &str = "application/json";
const APPLICATION_OCTET_STREAM: &str = "application/octet-stream";

/// Serialize trait for Restate services.
///
/// Default implementations are provided for primitives, and you can use the wrapper type [`Json`] to serialize using [`serde_json`].
///
/// This looks similar to [`serde::Serialize`], but allows to plug-in non-serde serialization formats (e.g. like Protobuf using `prost`).
pub trait Serialize {
    type Error: std::error::Error + Send + Sync + 'static;

    fn serialize(&self) -> Result<Bytes, Self::Error>;
}

// TODO perhaps figure out how to add a lifetime here so we can deserialize to borrowed types
/// Deserialize trait for Restate services.
///
/// Default implementations are provided for primitives, and you can use the wrapper type [`Json`] to serialize using [`serde_json`].
///
/// This looks similar to [`serde::Deserialize`], but allows to plug-in non-serde serialization formats (e.g. like Protobuf using `prost`).
pub trait Deserialize
where
    Self: Sized,
{
    type Error: std::error::Error + Send + Sync + 'static;

    fn deserialize(bytes: &mut Bytes) -> Result<Self, Self::Error>;
}

/// Trait encapsulating `content-type` information for the given serializer/deserializer.
///
/// This is used by service discovery to correctly specify the content type.
pub trait WithContentType {
    fn content_type() -> &'static str;
}

// --- Default implementation for Unit type

impl Serialize for () {
    type Error = Infallible;

    fn serialize(&self) -> Result<Bytes, Self::Error> {
        Ok(Bytes::new())
    }
}

impl Deserialize for () {
    type Error = Infallible;

    fn deserialize(_: &mut Bytes) -> Result<Self, Self::Error> {
        Ok(())
    }
}

impl WithContentType for () {
    fn content_type() -> &'static str {
        ""
    }
}

// --- Passthrough implementation

impl Serialize for Vec<u8> {
    type Error = Infallible;

    fn serialize(&self) -> Result<Bytes, Self::Error> {
        Ok(Bytes::copy_from_slice(self))
    }
}

impl Deserialize for Vec<u8> {
    type Error = Infallible;

    fn deserialize(b: &mut Bytes) -> Result<Self, Self::Error> {
        Ok(b.to_vec())
    }
}

impl WithContentType for Vec<u8> {
    fn content_type() -> &'static str {
        APPLICATION_OCTET_STREAM
    }
}

impl Serialize for Bytes {
    type Error = Infallible;

    fn serialize(&self) -> Result<Bytes, Self::Error> {
        Ok(self.clone())
    }
}

impl Deserialize for Bytes {
    type Error = Infallible;

    fn deserialize(b: &mut Bytes) -> Result<Self, Self::Error> {
        Ok(b.clone())
    }
}

impl WithContentType for Bytes {
    fn content_type() -> &'static str {
        APPLICATION_OCTET_STREAM
    }
}

// --- Primitives

macro_rules! impl_serde_primitives {
    ($ty:ty) => {
        impl Serialize for $ty {
            type Error = serde_json::Error;

            fn serialize(&self) -> Result<Bytes, Self::Error> {
                serde_json::to_vec(&self).map(Bytes::from)
            }
        }

        impl Deserialize for $ty {
            type Error = serde_json::Error;

            fn deserialize(bytes: &mut Bytes) -> Result<Self, Self::Error> {
                serde_json::from_slice(&bytes)
            }
        }

        impl WithContentType for $ty {
            fn content_type() -> &'static str {
                APPLICATION_JSON
            }
        }
    };
}

impl_serde_primitives!(String);
impl_serde_primitives!(u8);
impl_serde_primitives!(u16);
impl_serde_primitives!(u32);
impl_serde_primitives!(u64);
impl_serde_primitives!(u128);
impl_serde_primitives!(i8);
impl_serde_primitives!(i16);
impl_serde_primitives!(i32);
impl_serde_primitives!(i64);
impl_serde_primitives!(i128);
impl_serde_primitives!(bool);
impl_serde_primitives!(f32);
impl_serde_primitives!(f64);

// --- Json responses

/// Wrapper type to use [`serde_json`] with Restate's [`Serialize`]/[`Deserialize`] traits.
pub struct Json<T>(pub T);

impl<T> Json<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for Json<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> Serialize for Json<T>
where
    T: serde::Serialize,
{
    type Error = serde_json::Error;

    fn serialize(&self) -> Result<Bytes, Self::Error> {
        serde_json::to_vec(&self.0).map(Bytes::from)
    }
}

impl<T> Deserialize for Json<T>
where
    for<'a> T: serde::Deserialize<'a>,
{
    type Error = serde_json::Error;

    fn deserialize(bytes: &mut Bytes) -> Result<Self, Self::Error> {
        serde_json::from_slice(bytes).map(Json)
    }
}

impl<T: Default> Default for Json<T> {
    fn default() -> Self {
        Self(T::default())
    }
}
