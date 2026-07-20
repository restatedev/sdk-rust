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
mod struct_ast;
mod struct_generator;

use crate::ast::{Object, Service, ServiceType, Workflow};
use crate::generator::ServiceGenerator;
use crate::struct_ast::StructService;
use proc_macro::TokenStream;
use quote::ToTokens;
use syn::{Item, parse_macro_input};

#[proc_macro_attribute]
pub fn service(attr: TokenStream, input: TokenStream) -> TokenStream {
    dispatch(ServiceType::Service, attr, input)
}

#[proc_macro_attribute]
pub fn object(attr: TokenStream, input: TokenStream) -> TokenStream {
    dispatch(ServiceType::Object, attr, input)
}

#[proc_macro_attribute]
pub fn workflow(attr: TokenStream, input: TokenStream) -> TokenStream {
    dispatch(ServiceType::Workflow, attr, input)
}

/// Marks a method inside a `#[restate_sdk::service]`/`#[object]`/`#[workflow]` impl block as a
/// Restate handler.
///
/// This attribute is consumed by the enclosing service macro; on its own it is a no-op.
#[proc_macro_attribute]
pub fn handler(_: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Dispatch a service/object/workflow attribute macro to either the (deprecated) trait-based API or
/// the struct-based API, depending on whether it is applied to a `trait` or an `impl` block.
fn dispatch(service_ty: ServiceType, attr: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as Item);
    match item {
        Item::Impl(item_impl) => {
            let args = match struct_ast::parse_service_args(attr.into(), service_ty) {
                Ok(args) => args,
                Err(e) => return e.to_compile_error().into(),
            };
            match StructService::from_impl(service_ty, args, item_impl) {
                Ok(svc) => struct_generator::generate(&svc).into(),
                Err(e) => e.to_compile_error().into(),
            }
        }
        Item::Trait(item_trait) => {
            // Deprecated trait-based API. Re-parse the trait tokens through the legacy parser.
            let tokens = item_trait.into_token_stream();
            let result = match service_ty {
                ServiceType::Service => syn::parse2::<Service>(tokens)
                    .map(|s| ServiceGenerator::new_service(&s).into_token_stream()),
                ServiceType::Object => syn::parse2::<Object>(tokens)
                    .map(|s| ServiceGenerator::new_object(&s).into_token_stream()),
                ServiceType::Workflow => syn::parse2::<Workflow>(tokens)
                    .map(|s| ServiceGenerator::new_workflow(&s).into_token_stream()),
            };
            match result {
                Ok(ts) => ts.into(),
                Err(e) => e.to_compile_error().into(),
            }
        }
        other => syn::Error::new_spanned(
            other,
            "#[restate_sdk::service]/#[object]/#[workflow] can only be applied to a trait \
             (deprecated) or an inherent impl block",
        )
        .to_compile_error()
        .into(),
    }
}
