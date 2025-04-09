//! # Serialization
//!
//! Restate sends data over the network for storing state, journaling actions, awakeables, etc.
//!
//! Therefore, the types of the values that are stored, need to either:
//! - be a primitive type
//! - use a wrapper type [`Json`] for using [`serde-json`](https://serde.rs/)
//! - have the [`Serialize`] and [`Deserialize`] trait implemented
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

impl<T> WithContentType for Json<T> {
    fn content_type() -> &'static str {
        APPLICATION_JSON
    }
}

// -- Schema Generation

/// ## JSON Schema Generation
///
/// The SDK provides three approaches for generating JSON Schemas for handler inputs and outputs:
///
/// ### 1. Primitive Types
///
/// Primitive types (like `String`, `u32`, `bool`) have built-in schema implementations
/// that work automatically without additional code:
///
/// ```rust
/// use restate_sdk::prelude::*;
///
/// #[restate_sdk::service]
/// trait SimpleService {
///     async fn greet(name: String) -> HandlerResult<u32>;
/// }
/// ```
///
/// ### 2. Using Json<T> with schemars
///
/// For complex types wrapped in `Json<T>`, you need to add the `schemars` feature and derive `JsonSchema`:
///
/// ```rust
/// use restate_sdk::prelude::*;
///
/// #[derive(serde::Serialize, serde::Deserialize)]
/// #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
/// struct User {
///     name: String,
///     age: u32,
/// }
///
/// #[restate_sdk::service]
/// trait UserService {
///     async fn register(user: Json<User>) -> HandlerResult<Json<User>>;
/// }
/// ```
///
/// To enable rich schema generation with `Json<T>`, add the `schemars` feature to your dependency:
///
/// ```toml
/// [dependencies]
/// restate-sdk = { version = "0.3", features = ["schemars"] }
/// schemars = "1.0.0-alpha.17"
/// ```
///
/// ### 3. Custom Implementation
///
/// You can also implement the `WithSchema` trait directly for your types to provide
/// custom schemas without relying on the `schemars` feature:
///
/// ```rust
/// use restate_sdk::serde::{WithSchema, WithContentType, Serialize, Deserialize};
///
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct User {
///     name: String,
///     age: u32,
/// }
///
/// // Implement WithSchema directly
/// impl WithSchema for User {
///     fn generate_schema() -> serde_json::Value {
///         serde_json::json!({
///             "type": "object",
///             "properties": {
///                 "name": {"type": "string"},
///                 "age": {"type": "integer", "minimum": 0}
///             },
///             "required": ["name", "age"]
///         })
///     }
/// }
/// ```
///
/// Trait encapsulating JSON Schema information for the given serializer/deserializer.
///
/// This trait allows types to provide JSON Schema information that can be used for
/// documentation, validation, and client generation.
///
/// ## Behavior with `schemars` Feature Flag
///
/// When the `schemars` feature is enabled, implementations for complex types use
/// the `schemars` crate to automatically generate rich, JSON Schema 2020-12 conforming schemas.
/// When the feature is disabled, primitive types still provide basic schemas,
/// but complex types return empty schemas, unless manually implemented.
pub trait WithSchema {
    /// Generate a JSON Schema for this type.
    ///
    /// Returns a JSON value representing the schema for this type. When the `schemars`
    /// feature is enabled, this returns an auto-generated JSON Schema 2020-12 conforming schema. When the feature is disabled,
    /// this returns an empty schema for complex types, but basic schemas for primitives.
    fn generate_schema() -> serde_json::Value;
}

// Helper function to create an empty schema
fn empty_schema() -> serde_json::Value {
    serde_json::json!({})
}

// Basic implementations for primitive types (always available)

impl WithSchema for () {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "null" })
    }
}

impl WithSchema for String {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "string" })
    }
}

impl WithSchema for bool {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "boolean" })
    }
}

impl WithSchema for u8 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer", "minimum": 0, "maximum": 255 })
    }
}

impl WithSchema for u16 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer", "minimum": 0, "maximum": 65535 })
    }
}

impl WithSchema for u32 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer", "minimum": 0, "maximum": 4_294_967_295u64 })
    }
}

impl WithSchema for u64 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer", "minimum": 0 })
    }
}

impl WithSchema for u128 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer", "minimum": 0 })
    }
}

impl WithSchema for i8 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer", "minimum": -128, "maximum": 127 })
    }
}

impl WithSchema for i16 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer", "minimum": -32768, "maximum": 32767 })
    }
}

impl WithSchema for i32 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer", "minimum": -2147483648, "maximum": 2147483647 })
    }
}

impl WithSchema for i64 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer" })
    }
}

impl WithSchema for i128 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "integer" })
    }
}

impl WithSchema for f32 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "number" })
    }
}

impl WithSchema for f64 {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "number" })
    }
}

impl WithSchema for Vec<u8> {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "string", "format": "byte" })
    }
}

impl WithSchema for Bytes {
    fn generate_schema() -> serde_json::Value {
        serde_json::json!({ "type": "string", "format": "byte" })
    }
}

impl<T: WithSchema> WithSchema for Option<T> {
    fn generate_schema() -> serde_json::Value {
        T::generate_schema()
    }
}

// When schemars is disabled - works with any T
#[cfg(not(feature = "schemars"))]
impl<T> WithSchema for Json<T> {
    fn generate_schema() -> serde_json::Value {
        empty_schema() // Empty schema returns "accept all */*"
    }
}

// When schemars is enabled - requires T: JsonSchema
#[cfg(feature = "schemars")]
impl<T: schemars::JsonSchema> WithSchema for Json<T> {
    fn generate_schema() -> serde_json::Value {
        let schema = schemars::schema_for!(T);
        serde_json::to_value(schema).unwrap_or_else(|e| {
            tracing::debug!("Failed to convert schema to JSON: {}", e);
            empty_schema()
        })
    }
}
