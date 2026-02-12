// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

// Some parts copied from https://github.com/dtolnay/thiserror/blob/39aaeb00ff270a49e3c254d7b38b10e934d3c7a5/impl/src/ast.rs
// License Apache-2.0 or MIT

use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::token::Comma;
use syn::{
    Attribute, Error, Expr, ExprLit, FnArg, GenericArgument, Ident, Lit, Pat, PatType, Path,
    PathArguments, Result, ReturnType, Token, Type, Visibility, braced, parenthesized, parse_quote,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServiceType {
    Service,
    Object,
    Workflow,
}

pub(crate) struct Service(pub(crate) ServiceInner);

impl Parse for Service {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(Service(ServiceInner::parse(ServiceType::Service, input)?))
    }
}

pub(crate) struct Object(pub(crate) ServiceInner);

impl Parse for Object {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(Object(ServiceInner::parse(ServiceType::Object, input)?))
    }
}

pub(crate) struct Workflow(pub(crate) ServiceInner);

impl Parse for Workflow {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(Workflow(ServiceInner::parse(ServiceType::Workflow, input)?))
    }
}

pub(crate) struct ServiceInner {
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) restate_name: String,
    pub(crate) vis: Visibility,
    pub(crate) ident: Ident,
    pub(crate) handlers: Vec<Handler>,
}

impl ServiceInner {
    fn parse(service_type: ServiceType, input: ParseStream) -> Result<Self> {
        let parsed_attrs = input.call(Attribute::parse_outer)?;
        let vis = input.parse()?;
        input.parse::<Token![trait]>()?;
        let ident: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut rpcs = Vec::<Handler>::new();
        while !content.is_empty() {
            let h: Handler = content.parse()?;

            if h.is_shared && service_type == ServiceType::Service {
                return Err(Error::new(
                    h.ident.span(),
                    "Service handlers cannot be annotated with #[shared]",
                ));
            }

            rpcs.push(h);
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

        let mut attrs = vec![];
        let mut restate_name = ident.to_string();
        for attr in parsed_attrs {
            if let Some(name) = read_literal_attribute_name(&attr)? {
                restate_name = name;
            } else {
                // Just propagate
                attrs.push(attr);
            }
        }

        Ok(Self {
            attrs,
            restate_name,
            vis,
            ident,
            handlers: rpcs,
        })
    }
}

pub(crate) struct Handler {
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) is_shared: bool,
    pub(crate) is_lazy_state: bool,
    pub(crate) restate_name: String,
    pub(crate) ident: Ident,
    pub(crate) arg: Option<PatType>,
    pub(crate) output_ok: Type,
    pub(crate) output_err: Type,
}

impl Parse for Handler {
    fn parse(input: ParseStream) -> Result<Self> {
        let parsed_attrs = input.call(Attribute::parse_outer)?;

        input.parse::<Token![async]>()?;
        input.parse::<Token![fn]>()?;
        let ident: Ident = input.parse()?;

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
        let return_type: ReturnType = input.parse()?;
        input.parse::<Token![;]>()?;

        let (ok_ty, err_ty) = match &return_type {
            ReturnType::Default => {
                return Err(Error::new(
                    return_type.span(),
                    "The return type cannot be empty, only Result or restate_sdk::prelude::HandlerResult is supported as return type",
                ));
            }
            ReturnType::Type(_, ty) => {
                if let Some((ok_ty, err_ty)) = extract_handler_result_parameter(ty) {
                    (ok_ty, err_ty)
                } else {
                    return Err(Error::new(
                        return_type.span(),
                        "Only Result or restate_sdk::prelude::HandlerResult is supported as return type",
                    ));
                }
            }
        };

        // Process attributes
        let mut is_shared = false;
        let mut is_lazy_state = false;
        let mut restate_name = ident.to_string();
        let mut attrs = vec![];
        for attr in parsed_attrs {
            if is_shared_attr(&attr) {
                is_shared = true;
            } else if is_lazy_state_attr(&attr) {
                is_lazy_state = true;
            } else if let Some(name) = read_literal_attribute_name(&attr)? {
                restate_name = name;
            } else {
                // Just propagate
                attrs.push(attr);
            }
        }

        Ok(Self {
            attrs,
            is_shared,
            is_lazy_state,
            restate_name,
            ident,
            arg: args.pop(),
            output_ok: ok_ty,
            output_err: err_ty,
        })
    }
}

fn is_shared_attr(attr: &Attribute) -> bool {
    attr.meta
        .require_path_only()
        .and_then(Path::require_ident)
        .is_ok_and(|i| i == "shared")
}

fn is_lazy_state_attr(attr: &Attribute) -> bool {
    attr.meta
        .require_path_only()
        .and_then(Path::require_ident)
        .is_ok_and(|i| i == "lazy_state")
}

fn read_literal_attribute_name(attr: &Attribute) -> Result<Option<String>> {
    attr.meta
        .require_name_value()
        .ok()
        .filter(|val| val.path.require_ident().is_ok_and(|i| i == "name"))
        .map(|val| {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(literal),
                ..
            }) = &val.value
            {
                Ok(literal.value())
            } else {
                Err(Error::new(
                    val.span(),
                    "Only string literal is allowed for the 'name' attribute",
                ))
            }
        })
        .transpose()
}

fn extract_handler_result_parameter(ty: &Type) -> Option<(Type, Type)> {
    let path = match ty {
        Type::Path(ty) => &ty.path,
        _ => return None,
    };

    let last = path.segments.last().unwrap();
    let is_result = last.ident == "Result";
    let is_handler_result = last.ident == "HandlerResult";
    if !is_result && !is_handler_result {
        return None;
    }

    let bracketed = match &last.arguments {
        PathArguments::AngleBracketed(bracketed) => bracketed,
        _ => return None,
    };

    if is_handler_result && bracketed.args.len() == 1 {
        match &bracketed.args[0] {
            GenericArgument::Type(arg) => Some((
                arg.clone(),
                parse_quote!(::restate_sdk::prelude::HandlerError),
            )),
            _ => None,
        }
    } else if is_result && bracketed.args.len() == 2 {
        match (&bracketed.args[0], &bracketed.args[1]) {
            (GenericArgument::Type(ok_arg), GenericArgument::Type(err_arg)) => {
                Some((ok_arg.clone(), err_arg.clone()))
            }
            _ => None,
        }
    } else {
        None
    }
}
