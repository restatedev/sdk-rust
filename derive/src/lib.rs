// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Some parts of this codebase were taken from https://github.com/dtolnay/thiserror/tree/39aaeb00ff270a49e3c254d7b38b10e934d3c7a5/impl/src
//! License Apache-2.0 or MIT

extern crate proc_macro;

mod ast;
mod attr;
mod expand;
mod generics;
mod prop;
mod valid;

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

// TODO macro
// struct MyStruct;
//
// impl Service for MyStruct {
//     fn discover() -> ServiceDefinition {
//         todo!()
//     }
//
//     fn handle(&self, svc_name: &str, handler_name: &str, ctx: Context) -> impl Future + Send + 'static {
//         todo!()
//     }
// }
//
// trait MyTrait {}
//
// impl<T: MyTrait> Service for T {
//     fn discover() -> ServiceDefinition {
//         todo!()
//     }
//
//     fn handle(&self, svc_name: &str, handler_name: &str, ctx: Context) -> impl Future + Send + 'static {
//         todo!()
//     }
// }

#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::Item);
    expand::derive(&input)
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

// #[proc_macro_attribute]
// pub fn virtual_object(attr: TokenStream, item: TokenStream) -> TokenStream {
//     let input = parse_macro_input!(input as DeriveInput);
//     expand::derive(&input)
//         .unwrap_or_else(|err| err.to_compile_error())
//         .into()
// }
//
// #[proc_macro_attribute]
// pub fn workflow(attr: TokenStream, item: TokenStream) -> TokenStream {
//     let input = parse_macro_input!(input as DeriveInput);
//     expand::derive(&input)
//         .unwrap_or_else(|err| err.to_compile_error())
//         .into()
// }
