use std::time::Duration;

use crate::serde::Serialize;

use super::{internal::IngressClientError, IngressInternal};

pub struct IngressAwakeable<'a> {
    inner: &'a IngressInternal,
    key: String,
    opts: IngressAwakeableOptions,
}

#[derive(Default, Clone)]
pub(super) struct IngressAwakeableOptions {
    pub(super) timeout: Option<Duration>,
}

impl<'a> IngressAwakeable<'a> {
    pub(super) fn new(inner: &'a IngressInternal, key: impl Into<String>) -> Self {
        Self {
            inner,
            key: key.into(),
            opts: Default::default(),
        }
    }

    /// Set the timeout for the request.
    pub fn timeout(mut self, value: Duration) -> Self {
        self.opts.timeout = Some(value);
        self
    }

    /// Resolve the awakeable with a payload
    pub async fn resolve<T: Serialize + 'static>(
        self,
        payload: T,
    ) -> Result<(), IngressClientError> {
        self.inner
            .resolve_awakeable(&self.key, payload, self.opts)
            .await
    }

    /// Reject the awakeable with a failure message
    pub async fn reject(self, message: impl Into<String>) -> Result<(), IngressClientError> {
        self.inner
            .reject_awakeable(&self.key, &message.into(), self.opts)
            .await
    }
}
