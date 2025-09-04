//! This module contains the generated data structures from the [service protocol manifest schema](https://github.com/restatedev/service-protocol/blob/main/endpoint_manifest_schema.json).

mod generated {
    #![allow(clippy::clone_on_copy)]
    #![allow(clippy::to_string_trait_impl)]

    include!(concat!(env!("OUT_DIR"), "/endpoint_manifest.rs"));
}

use crate::endpoint::{HandlerOptions, ServiceOptions};
use crate::serde::PayloadMetadata;
pub use generated::*;

impl InputPayload {
    pub fn empty() -> Self {
        Self {
            content_type: None,
            json_schema: None,
            required: None,
        }
    }

    pub fn from_metadata<T: PayloadMetadata>() -> Self {
        let input_metadata = T::input_metadata();
        Self {
            content_type: Some(input_metadata.accept_content_type.to_owned()),
            json_schema: T::json_schema(),
            required: Some(input_metadata.is_required),
        }
    }
}

impl OutputPayload {
    pub fn empty() -> Self {
        Self {
            content_type: None,
            json_schema: None,
            set_content_type_if_empty: Some(false),
        }
    }

    pub fn from_metadata<T: PayloadMetadata>() -> Self {
        let output_metadata = T::output_metadata();
        Self {
            content_type: Some(output_metadata.content_type.to_owned()),
            json_schema: T::json_schema(),
            set_content_type_if_empty: Some(output_metadata.set_content_type_if_empty),
        }
    }
}

impl Service {
    pub(crate) fn apply_options(&mut self, options: ServiceOptions) {
        // Apply service options
        self.metadata.extend(options.metadata);
        self.inactivity_timeout = options.inactivity_timeout.map(|d| d.as_millis() as u64);
        self.abort_timeout = options.abort_timeout.map(|d| d.as_millis() as u64);
        self.inactivity_timeout = options.inactivity_timeout.map(|d| d.as_millis() as u64);
        self.idempotency_retention = options.idempotency_retention.map(|d| d.as_millis() as u64);
        self.journal_retention = options.journal_retention.map(|d| d.as_millis() as u64);
        self.enable_lazy_state = options.enable_lazy_state;
        self.ingress_private = options.ingress_private;
        // Retry policy
        self.retry_policy_initial_interval = options
            .retry_policy_initial_interval
            .map(|d| d.as_millis() as u64);
        self.retry_policy_exponentiation_factor = options.retry_policy_exponentiation_factor;
        self.retry_policy_max_interval = options
            .retry_policy_max_interval
            .map(|d| d.as_millis() as u64);
        self.retry_policy_max_attempts = options.retry_policy_max_attempts;
        self.retry_policy_on_max_attempts = options.retry_policy_on_max_attempts;

        // Apply handler specific options
        for (handler_name, handler_options) in options.handler_options {
            let handler = self
                .handlers
                .iter_mut()
                .find(|h| handler_name == h.name.as_str())
                .expect("Invalid handler name provided in the options");
            handler.apply_options(handler_options);
        }
    }
}

impl Handler {
    pub(crate) fn apply_options(&mut self, options: HandlerOptions) {
        // Apply handler options
        self.metadata.extend(options.metadata);
        self.inactivity_timeout = options.inactivity_timeout.map(|d| d.as_millis() as u64);
        self.abort_timeout = options.abort_timeout.map(|d| d.as_millis() as u64);
        self.inactivity_timeout = options.inactivity_timeout.map(|d| d.as_millis() as u64);
        self.idempotency_retention = options.idempotency_retention.map(|d| d.as_millis() as u64);
        self.journal_retention = options.journal_retention.map(|d| d.as_millis() as u64);
        self.workflow_completion_retention =
            options.workflow_retention.map(|d| d.as_millis() as u64);
        self.enable_lazy_state = options.enable_lazy_state;
        self.ingress_private = options.ingress_private;
        // Retry policy
        self.retry_policy_initial_interval = options
            .retry_policy_initial_interval
            .map(|d| d.as_millis() as u64);
        self.retry_policy_exponentiation_factor = options.retry_policy_exponentiation_factor;
        self.retry_policy_max_interval = options
            .retry_policy_max_interval
            .map(|d| d.as_millis() as u64);
        self.retry_policy_max_attempts = options.retry_policy_max_attempts;
        self.retry_policy_on_max_attempts = options.retry_policy_on_max_attempts;
    }
}
