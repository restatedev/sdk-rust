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
mod gen;

use crate::ast::{Object, Service, ServiceInner, ValidArgs, Workflow};
use crate::gen::ServiceGenerator;
use proc_macro::TokenStream;
use quote::ToTokens;
use syn::parse_macro_input;

#[proc_macro_attribute]
pub fn service(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut svc = parse_macro_input!(input as Service);

    match &mut svc.0 {
        ServiceInner::Trait(..) => {}
        ServiceInner::Impl(inner) => {
            let args = parse_macro_input!(args as ValidArgs);
            inner.restate_name = args.restate_name.unwrap_or(inner.ident.to_string());
            inner.vis = args.vis;
        }
    }

    ServiceGenerator::new_service(&svc)
        .into_token_stream()
        .into()
}

#[proc_macro_attribute]
pub fn object(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut svc = parse_macro_input!(input as Object);

    match &mut svc.0 {
        ServiceInner::Trait(..) => {}
        ServiceInner::Impl(inner) => {
            let args = parse_macro_input!(args as ValidArgs);
            inner.restate_name = args.restate_name.unwrap_or(inner.ident.to_string());
            inner.vis = args.vis;
        }
    }

    ServiceGenerator::new_object(&svc)
        .into_token_stream()
        .into()
}

#[proc_macro_attribute]
pub fn workflow(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut svc = parse_macro_input!(input as Workflow);

    match &mut svc.0 {
        ServiceInner::Trait(..) => {}
        ServiceInner::Impl(inner) => {
            let args = parse_macro_input!(args as ValidArgs);
            inner.restate_name = args.restate_name.unwrap_or(inner.ident.to_string());
            inner.vis = args.vis;
        }
    }

    ServiceGenerator::new_workflow(&svc)
        .into_token_stream()
        .into()
}
