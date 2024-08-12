// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Some parts copied from https://github.com/dtolnay/thiserror/blob/39aaeb00ff270a49e3c254d7b38b10e934d3c7a5/impl/src/ast.rs
//! License Apache-2.0 or MIT

use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::token::Comma;
use syn::{
    braced, parenthesized, Attribute, Error, FnArg, GenericArgument, Ident, Pat, PatType, Path,
    PathArguments, Result, ReturnType, Token, Type, Visibility,
};

/// Accumulates multiple errors into a result.
/// Only use this for recoverable errors, i.e. non-parse errors. Fatal errors should early exit to
/// avoid further complications.
macro_rules! extend_errors {
    ($errors: ident, $e: expr) => {
        match $errors {
            Ok(_) => $errors = Err($e),
            Err(ref mut errors) => errors.extend($e),
        }
    };
}

pub(crate) struct Service {
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) vis: Visibility,
    pub(crate) ident: Ident,
    pub(crate) handlers: Vec<Handler>,
}

impl Parse for Service {
    fn parse(input: ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis = input.parse()?;
        input.parse::<Token![trait]>()?;
        let ident: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut rpcs = Vec::<Handler>::new();
        while !content.is_empty() {
            rpcs.push(content.parse()?);
        }
        let mut ident_errors = Ok(());
        for rpc in &rpcs {
            if rpc.ident == "new" {
                extend_errors!(
                    ident_errors,
                    Error::new(
                        rpc.ident.span(),
                        format!(
                            "method name conflicts with generated fn `{}Client::new`",
                            ident.unraw()
                        )
                    )
                );
            }
            if rpc.ident == "serve" {
                extend_errors!(
                    ident_errors,
                    Error::new(
                        rpc.ident.span(),
                        format!("method name conflicts with generated fn `{ident}::serve`")
                    )
                );
            }
        }
        ident_errors?;

        Ok(Self {
            attrs,
            vis,
            ident,
            handlers: rpcs,
        })
    }
}

pub(crate) struct Handler {
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) is_shared: bool,
    pub(crate) ident: Ident,
    pub(crate) arg: Option<PatType>,
    pub(crate) output: ReturnType,
}

impl Parse for Handler {
    fn parse(input: ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let parsed_attrs_count = attrs.len();

        // Remove shared attribute
        let attrs: Vec<Attribute> = attrs.into_iter().filter(|a| !is_shared_attr(a)).collect();
        let is_shared = attrs.len() != parsed_attrs_count;

        input.parse::<Token![async]>()?;
        input.parse::<Token![fn]>()?;
        let ident = input.parse()?;

        // Parse arguments
        let content;
        parenthesized!(content in input);
        let mut args = Vec::new();
        let mut errors = Ok(());
        for arg in content.parse_terminated(FnArg::parse, Comma)? {
            match arg {
                FnArg::Typed(captured) if matches!(&*captured.pat, Pat::Ident(_)) => {
                    args.push(captured);
                }
                FnArg::Typed(captured) => {
                    extend_errors!(
                        errors,
                        Error::new(captured.pat.span(), "patterns aren't allowed in RPC args")
                    );
                }
                FnArg::Receiver(_) => {
                    extend_errors!(
                        errors,
                        Error::new(arg.span(), "method args cannot start with self")
                    );
                }
            }
        }
        if args.len() > 1 {
            extend_errors!(
                errors,
                Error::new(content.span(), "Only one input argument is supported")
            );
        }
        errors?;

        // Parse return type
        let output: ReturnType = input.parse()?;
        input.parse::<Token![;]>()?;

        match &output {
            ReturnType::Default => {}
            ReturnType::Type(_, ty) => {
                if handler_result_parameter(ty).is_none() {
                    return Err(Error::new(
                        output.span(),
                        "Only restate_sdk::prelude::HandlerResult is supported as return type",
                    ));
                }
            }
        }

        Ok(Self {
            attrs,
            is_shared,
            ident,
            arg: args.pop(),
            output,
        })
    }
}

fn is_shared_attr(attr: &Attribute) -> bool {
    attr.meta
        .require_path_only()
        .and_then(Path::require_ident)
        .is_ok_and(|i| i == "shared")
}

fn handler_result_parameter(ty: &Type) -> Option<&Type> {
    let path = match ty {
        Type::Path(ty) => &ty.path,
        _ => return None,
    };

    let last = path.segments.last().unwrap();
    if last.ident != "HandlerResult" {
        return None;
    }

    let bracketed = match &last.arguments {
        PathArguments::AngleBracketed(bracketed) => bracketed,
        _ => return None,
    };

    if bracketed.args.len() != 1 {
        return None;
    }

    match &bracketed.args[0] {
        GenericArgument::Type(arg) => Some(arg),
        _ => None,
    }
}
