use bytes::Bytes;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct Store {
    journal: Rc<RefCell<super::journal::Journal>>,
    values: RefCell<HashMap<String, Bytes>>,
}

impl Store {
    pub fn get<T: Deserialize>(&self, key: &str) -> Result<Option<T>, T::Error> {
        self.values.borrow().get(key).map(|b| T::deserialize(b)).transpose()
    }

    pub fn get_or_default<T: Deserialize + Default>(&self, key: &str) -> Result<T, T::Error> {
        self.values.borrow().get(key)
            .map(|b| T::deserialize(b))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub fn set<T: Serialize>(&self, key: impl Into<String>, value: T) -> Result<(), T::Error> {
        self.values.borrow_mut().insert(key.into(), value.serialize()?);
        Ok(())
    }

    pub fn clear(&self, key: &str) {
        self.values.borrow_mut().remove(key);
    }
}

pub trait Serialize {
    type Error;

    fn serialize(&self) -> Result<Bytes, Self::Error>;
}

pub trait Deserialize
where
    Self: Sized,
{
    type Error;

    fn deserialize(bytes: &[u8]) -> Result<Self, Self::Error>;
}

mod prost_impl {
    use super::*;

    use prost::Message;

    impl<T> Serialize for T
    where
        T: Message,
    {
        type Error = ();

        fn serialize(&self) -> Result<Bytes, Self::Error> {
            Ok(self.encode_to_vec().into())
        }
    }

    impl<T> Deserialize for T
    where
        T: Message + Default,
    {
        type Error = prost::DecodeError;

        fn deserialize(bytes: &[u8]) -> Result<Self, Self::Error> {
            T::decode(bytes)
        }
    }
}

#[cfg(feature = "state-serde-cbor")]
mod serde_cbor_impl {
    use super::*;

    impl<T> Serialize for T
    where
        T: serde::Serialize,
    {
        type Error = serde_cbor::Error;

        fn serialize(&self) -> Result<Bytes, Self::Error> {
            serde_cbor::to_vec(self).map(Into::into)
        }
    }

    impl<T> Deserialize for T
    where
        T: serde::Deserialize,
    {
        type Error = serde_cbor::Error;

        fn deserialize(bytes: &[u8]) -> Result<T, Self::Error> {
            serde_cbor::from_slice(bytes)
        }
    }
}
