//! # Serialization
//!
//! Restate sends data over the network for storing state, journaling actions, awakeables, etc.
//!
//! Therefore, the types of the values that are stored, need to either:
//! - be a primitive type
//! - use a wrapper type [`Json`] for using [`serde-json`](https://serde.rs/). To enable JSON schema generation, you'll need to enable the `schemars` feature. See [PayloadMetadata] for more details.
//! - have the [`Serialize`] and [`Deserialize`] trait implemented. If you need to use a type for the handler input/output, you'll also need to implement [PayloadMetadata] to reply with correct content type and enable **JSON schema generation**.
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

/// ## Payload metadata and Json Schemas
///
/// The SDK propagates during discovery some metadata to restate-server service catalog. This includes:
///
/// * The JSON schema of the payload. See below for more details.
/// * The [InputMetadata] used to instruct restate how to accept requests.
/// * The [OutputMetadata] used to instruct restate how to send responses out.
///
/// There are three approaches for generating JSON Schemas for handler inputs and outputs:
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
/// ### 2. Using `Json<T>` with schemars
///
/// For complex types wrapped in `Json<T>`, you need to add the `schemars` feature and derive `JsonSchema`:
///
/// ```rust
/// use restate_sdk::prelude::*;
///
/// #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
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
/// You can also implement the [PayloadMetadata] trait directly for your types to provide
/// custom schemas without relying on the `schemars` feature:
///
/// ```rust
/// use restate_sdk::serde::{PayloadMetadata, Serialize, Deserialize};
///
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct User {
///     name: String,
///     age: u32,
/// }
///
/// // Implement PayloadMetadata directly and override the json_schema implementation
/// impl PayloadMetadata for User {
///     fn json_schema() -> Option<serde_json::Value> {
///         Some(serde_json::json!({
///             "type": "object",
///             "properties": {
///                 "name": {"type": "string"},
///                 "age": {"type": "integer", "minimum": 0}
///             },
///             "required": ["name", "age"]
///         }))
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
pub trait PayloadMetadata {
    /// Generate a JSON Schema for this type.
    ///
    /// Returns a JSON value representing the schema for this type. When the `schemars`
    /// feature is enabled, this returns an auto-generated JSON Schema 2020-12 conforming schema. When the feature is disabled,
    /// this returns an empty schema for complex types, but basic schemas for primitives.
    ///
    /// If returns none, no schema is provided. This should be used when the payload is not expected to be json
    fn json_schema() -> Option<serde_json::Value> {
        Some(serde_json::Value::Object(serde_json::Map::default()))
    }

    /// Returns the [InputMetadata]. The default implementation returns metadata suitable for JSON payloads.
    fn input_metadata() -> InputMetadata {
        InputMetadata::default()
    }

    /// Returns the [OutputMetadata]. The default implementation returns metadata suitable for JSON payloads.
    fn output_metadata() -> OutputMetadata {
        OutputMetadata::default()
    }
}

/// This struct encapsulates input payload metadata used by discovery.
///
/// The default implementation works well with Json payloads.
pub struct InputMetadata {
    /// Content type of the input. It can accept wildcards, in the same format as the 'Accept' header.
    ///
    /// By default, is `application/json`.
    pub accept_content_type: &'static str,
    /// If true, Restate itself will reject requests **without content-types**.
    pub is_required: bool,
}

impl Default for InputMetadata {
    fn default() -> Self {
        Self {
            accept_content_type: APPLICATION_JSON,
            is_required: true,
        }
    }
}

/// This struct encapsulates output payload metadata used by discovery.
///
/// The default implementation works for Json payloads.
pub struct OutputMetadata {
    /// Content type of the output.
    ///
    /// By default, is `application/json`.
    pub content_type: &'static str,
    /// If true, the specified content-type is set even if the output is empty. This should be set to `true` only for encodings that can return a serialized empty byte array (e.g. Protobuf).
    pub set_content_type_if_empty: bool,
}

impl Default for OutputMetadata {
    fn default() -> Self {
        Self {
            content_type: APPLICATION_JSON,
            set_content_type_if_empty: false,
        }
    }
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

impl PayloadMetadata for Vec<u8> {
    fn json_schema() -> Option<serde_json::Value> {
        None
    }

    fn input_metadata() -> InputMetadata {
        InputMetadata {
            accept_content_type: "*/*",
            is_required: true,
        }
    }

    fn output_metadata() -> OutputMetadata {
        OutputMetadata {
            content_type: APPLICATION_OCTET_STREAM,
            set_content_type_if_empty: false,
        }
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

impl PayloadMetadata for Bytes {
    fn json_schema() -> Option<serde_json::Value> {
        None
    }

    fn input_metadata() -> InputMetadata {
        InputMetadata {
            accept_content_type: "*/*",
            is_required: true,
        }
    }

    fn output_metadata() -> OutputMetadata {
        OutputMetadata {
            content_type: APPLICATION_OCTET_STREAM,
            set_content_type_if_empty: false,
        }
    }
}
// --- Option implementation

impl<T: Serialize> Serialize for Option<T> {
    type Error = T::Error;

    fn serialize(&self) -> Result<Bytes, Self::Error> {
        if self.is_none() {
            return Ok(Bytes::new());
        }
        T::serialize(self.as_ref().unwrap())
    }
}

impl<T: Deserialize> Deserialize for Option<T> {
    type Error = T::Error;

    fn deserialize(b: &mut Bytes) -> Result<Self, Self::Error> {
        if b.is_empty() {
            return Ok(None);
        }
        T::deserialize(b).map(Some)
    }
}

impl<T: PayloadMetadata> PayloadMetadata for Option<T> {
    fn input_metadata() -> InputMetadata {
        InputMetadata {
            accept_content_type: T::input_metadata().accept_content_type,
            is_required: false,
        }
    }

    fn output_metadata() -> OutputMetadata {
        OutputMetadata {
            content_type: T::output_metadata().content_type,
            set_content_type_if_empty: false,
        }
    }
}

// --- Primitives

macro_rules! impl_integer_primitives {
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

        impl PayloadMetadata for $ty {
            fn json_schema() -> Option<serde_json::Value> {
                let min = <$ty>::MIN;
                let max = <$ty>::MAX;
                Some(serde_json::json!({ "type": "integer", "minimum": min, "maximum": max }))
            }
        }
    };
}

impl_integer_primitives!(u8);
impl_integer_primitives!(u16);
impl_integer_primitives!(u32);
impl_integer_primitives!(u64);
impl_integer_primitives!(u128);
impl_integer_primitives!(i8);
impl_integer_primitives!(i16);
impl_integer_primitives!(i32);
impl_integer_primitives!(i64);
impl_integer_primitives!(i128);

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
    };
}

impl_serde_primitives!(String);
impl_serde_primitives!(bool);
impl_serde_primitives!(f32);
impl_serde_primitives!(f64);

impl PayloadMetadata for String {
    fn json_schema() -> Option<serde_json::Value> {
        Some(serde_json::json!({ "type": "string" }))
    }
}

impl PayloadMetadata for bool {
    fn json_schema() -> Option<serde_json::Value> {
        Some(serde_json::json!({ "type": "boolean" }))
    }
}

impl PayloadMetadata for f32 {
    fn json_schema() -> Option<serde_json::Value> {
        Some(serde_json::json!({ "type": "number" }))
    }
}

impl PayloadMetadata for f64 {
    fn json_schema() -> Option<serde_json::Value> {
        Some(serde_json::json!({ "type": "number" }))
    }
}

// --- Json wrapper

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

// When schemars is disabled - works with any T
#[cfg(not(feature = "schemars"))]
impl<T> PayloadMetadata for Json<T> {
    fn json_schema() -> Option<serde_json::Value> {
        Some(serde_json::json!({}))
    }
}

// When schemars is enabled - requires T: JsonSchema
#[cfg(feature = "schemars")]
impl<T: schemars::JsonSchema> PayloadMetadata for Json<T> {
    fn json_schema() -> Option<serde_json::Value> {
        Some(schemars::schema_for!(T).to_value())
    }
}
