// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

// Some parts of this codebase were taken from https://github.com/google/tarpc/blob/b826f332312d3702667880a464e247556ad7dbfe/plugins/src/lib.rs
// License MIT

extern crate proc_macro;

mod ast;
mod generator;
mod handler;
mod names;
mod service_def;

use crate::ast::{Object, Service, Workflow};
use crate::generator::ServiceGenerator;
use proc_macro::TokenStream;
use quote::ToTokens;
use syn::parse_macro_input;

#[proc_macro_attribute]
pub fn service(_: TokenStream, input: TokenStream) -> TokenStream {
    let svc = parse_macro_input!(input as Service);

    ServiceGenerator::new_service(&svc)
        .into_token_stream()
        .into()
}

#[proc_macro_attribute]
pub fn object(_: TokenStream, input: TokenStream) -> TokenStream {
    let svc = parse_macro_input!(input as Object);

    ServiceGenerator::new_object(&svc)
        .into_token_stream()
        .into()
}

#[proc_macro_attribute]
pub fn workflow(_: TokenStream, input: TokenStream) -> TokenStream {
    let svc = parse_macro_input!(input as Workflow);

    ServiceGenerator::new_workflow(&svc)
        .into_token_stream()
        .into()
}

/// Turn a free `async fn` into a Restate handler value.
///
/// See [`restate_sdk::handler`](../restate_sdk/attr.handler.html) for documentation.
#[proc_macro_attribute]
pub fn handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    handler::expand(attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Declaratively define a service (exposed as `service!` via the prelude). See
/// [`restate_sdk::service`](../restate_sdk/macro.service.html).
#[proc_macro]
pub fn define_service(input: TokenStream) -> TokenStream {
    service_def::expand(service_def::Kind::Service, input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Declaratively define a virtual object (exposed as `object!` via the prelude).
#[proc_macro]
pub fn define_object(input: TokenStream) -> TokenStream {
    service_def::expand(service_def::Kind::Object, input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Declaratively define a workflow (exposed as `workflow!` via the prelude).
#[proc_macro]
pub fn define_workflow(input: TokenStream) -> TokenStream {
    service_def::expand(service_def::Kind::Workflow, input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
