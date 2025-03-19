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

use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{
    parse_quote, Attribute, Error, Expr, ExprLit, FnArg, GenericArgument, Ident, ImplItem,
    ImplItemFn, ItemImpl, Lit, Meta, Pat, PatType, PathArguments, Result, ReturnType, Type,
    Visibility,
};

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

pub(crate) struct ValidArgs {
    pub(crate) vis: Visibility,
    pub(crate) restate_name: Option<String>,
}

impl Parse for ValidArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut vis = None;
        let mut restate_name = None;

        let punctuated =
            syn::punctuated::Punctuated::<Meta, syn::token::Comma>::parse_terminated(input)?;

        for meta in punctuated {
            match meta {
                Meta::NameValue(name_value) if name_value.path.is_ident("vis") => {
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(lit_str),
                        ..
                    }) = &name_value.value
                    {
                        let vis_str = lit_str.value();
                        vis = Some(syn::parse_str::<Visibility>(&vis_str).map_err(|e| {
                            Error::new(
                                name_value.value.span(),
                                format!(
                                    "Invalid visibility modifier '{}'. Expected \"pub\", \"pub(crate)\", etc.: {}",
                                    vis_str, e
                                ),
                            )
                        })?);
                    } else {
                        return Err(Error::new(
                                name_value.value.span(),
                                "Expected a string literal for 'vis' (e.g., vis = \"pub\", vis = \"pub(crate)\")",
                        ));
                    }
                }
                Meta::NameValue(name_value) if name_value.path.is_ident("name") => {
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Str(lit_str),
                        ..
                    }) = &name_value.value
                    {
                        restate_name = Some(lit_str.value());
                    } else {
                        return Err(Error::new(
                            name_value.span(),
                            "Expected a string literal for 'name'",
                        ));
                    }
                }
                bad_meta => {
                    return Err(Error::new(
                        bad_meta.span(),
                        "Invalid attribute format. Expected #[service(vis = pub(crate), name = \"...\")]",
                    ));
                }
            }
        }

        Ok(Self {
            vis: vis.unwrap_or(Visibility::Inherited),
            restate_name,
        })
    }
}

// TODO: allow the user to have unused context like _:Context in the handler

// TODO: what should happen if having that thing on multiple blocks?

pub(crate) struct ServiceInner {
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) restate_name: String,
    pub(crate) ident: Ident,
    pub(crate) vis: Visibility,
    pub(crate) impl_block: ItemImpl,
    pub(crate) handlers: Vec<Handler>,
}

pub(crate) struct Handler {
    pub(crate) is_shared: bool,
    pub(crate) restate_name: String,
    pub(crate) ident: Ident,
    pub(crate) arg: Option<PatType>,
    pub(crate) output_ok: Type,
    #[allow(dead_code)]
    pub(crate) output_err: Type,
}

impl ServiceInner {
    fn parse(service_type: ServiceType, input: ParseStream) -> Result<Self> {
        let mut impl_block = input.parse::<ItemImpl>()?;
        let ident = match impl_block.self_ty.as_ref() {
            Type::Path(path) => path.path.segments[0].ident.clone(),
            bad_path => {
                return Err(Error::new(bad_path.span(), format!("Only on impl blocks")));
            }
        };

        let mut rpcs = Vec::new();
        for item in impl_block.items.iter_mut() {
            match item {
                ImplItem::Const(_) => {}
                ImplItem::Fn(handler) => {
                    let mut is_handler = false;
                    let mut is_shared = false;
                    let mut restate_name = None;

                    let mut attrs = Vec::with_capacity(handler.attrs.len());
                    for attr in &handler.attrs {
                        if attr.path().is_ident("handler") {
                            if is_handler {
                                return Err(Error::new(
                                    attr.span(),
                                    "Multiple `#[handler]` attributes found.",
                                ));
                            }
                            if handler.sig.asyncness.is_none() {
                                return Err(Error::new(
                                    handler.sig.fn_token.span(),
                                    "expected async, handlers are async fn",
                                ));
                            }
                            is_handler = true;
                            (is_shared, restate_name) =
                                extract_handler_attributes(service_type, attr)?;
                        } else {
                            attrs.push(attr.clone());
                        }
                    }

                    if is_handler {
                        let handler_arg =
                            validate_handler_arguments(service_type, is_shared, handler)?;

                        let return_type: ReturnType = handler.sig.output.clone();
                        let (output_ok, output_err) = match &return_type {
                            ReturnType::Default => {
                                return Err(Error::new(
                                        return_type.span(),
                                        "The return type cannot be empty, only Result or restate_sdk::prelude::HandlerResult is supported as return type",
                                ));
                            }
                            ReturnType::Type(_, ty) => {
                                if let Some((ok_ty, err_ty)) = extract_handler_result_parameter(ty)
                                {
                                    (ok_ty, err_ty)
                                } else {
                                    return Err(Error::new(
                                            return_type.span(),
                                            "Only Result or restate_sdk::prelude::HandlerResult is supported as return type",
                                    ));
                                }
                            }
                        };

                        handler.attrs = attrs;

                        rpcs.push(Handler {
                            is_shared,
                            ident: handler.sig.ident.clone(),
                            restate_name: restate_name.unwrap_or(handler.sig.ident.to_string()),
                            arg: handler_arg,
                            output_ok,
                            output_err,
                        });
                    }
                }
                bad_impl_item => {
                    return Err(Error::new(
                        bad_impl_item.span(),
                        format!("Only on consts and fns"),
                    ));
                }
            }
        }

        Ok(Self {
            attrs: impl_block.attrs.clone(),
            restate_name: "".to_string(),
            ident,
            vis: Visibility::Inherited,
            impl_block,
            handlers: rpcs,
        })
    }
}

fn extract_handler_attributes(
    service_type: ServiceType,
    attr: &Attribute,
) -> Result<(bool, Option<String>)> {
    let mut is_shared = false;
    let mut restate_name = None;

    match &attr.meta {
        Meta::Path(_) => {}
        Meta::List(meta_list) => {
            let mut seen_shared = false;
            let mut seen_name = false;
            meta_list.parse_nested_meta(|meta| {
                if meta.path.is_ident("shared") {
                    if seen_shared {
                        return Err(Error::new(meta.path.span(), "Duplicate `shared`"));
                    }
                    if service_type == ServiceType::Service {
                        return Err(Error::new(
                            meta.path.span(),
                            "Service handlers cannot be annotated with #[handler(shared)]",
                        ));
                    }
                    is_shared = true;
                    seen_shared = true;
                } else if meta.path.is_ident("name") {
                    if seen_name {
                        return Err(Error::new(meta.path.span(), "Duplicate `name`"));
                    }
                    let lit: Lit = meta.value()?.parse()?;
                    if let Lit::Str(lit_str) = lit {
                        seen_name = true;
                        restate_name = Some(lit_str.value());
                    } else {
                        return Err(Error::new(
                            lit.span(),
                            "Expected `name` to be a string literal",
                        ));
                    }
                } else {
                    return Err(Error::new(
                        meta.path.span(),
                        "Invalid attribute inside #[handler]",
                    ));
                }
                Ok(())
            })?;
        }
        Meta::NameValue(_) => {
            return Err(Error::new(
                attr.meta.span(),
                "Invalid attribute format for #[handler]",
            ));
        }
    }
    Ok((is_shared, restate_name))
}

fn validate_handler_arguments(
    service_type: ServiceType,
    is_shared: bool,
    handler: &ImplItemFn,
) -> Result<Option<PatType>> {
    let mut args_iter = handler.sig.inputs.iter();

    match args_iter.next() {
        Some(FnArg::Receiver(_)) => {}
        Some(arg) => {
            return Err(Error::new(
                arg.span(),
                "handler should have a `self` argument",
            ));
        }
        None => {
            return Err(Error::new(
                handler.sig.ident.span(),
                "Invalid handler arguments. It should be like (`self`, `ctx`, optional arg)",
            ));
        }
    };

    let valid_ctx: Ident = match (&service_type, is_shared) {
        (ServiceType::Service, _) => parse_quote! { Context },
        (ServiceType::Object, true) => parse_quote! { SharedObjectContext },
        (ServiceType::Object, false) => parse_quote! { ObjectContext },
        (ServiceType::Workflow, true) => parse_quote! { SharedWorkflowContext },
        (ServiceType::Workflow, false) => parse_quote! { WorkflowContext },
    };

    match args_iter.next() {
        Some(arg @ FnArg::Typed(typed_arg)) if matches!(&*typed_arg.pat, Pat::Ident(_)) => {
            if let Type::Path(type_path) = &*typed_arg.ty {
                let ctx_ident = &type_path.path.segments.last().unwrap().ident;

                if ctx_ident != &valid_ctx {
                    let service_desc = match service_type {
                        ServiceType::Service => "service",
                        ServiceType::Object => {
                            if is_shared {
                                "shared object"
                            } else {
                                "object"
                            }
                        }
                        ServiceType::Workflow => {
                            if is_shared {
                                "shared workflow"
                            } else {
                                "workflow"
                            }
                        }
                    };

                    return Err(Error::new(
                        ctx_ident.span(),
                        format!(
                            "Expects `{}` type for this `{}`, but `{}` was provided.",
                            valid_ctx, service_desc, ctx_ident
                        ),
                    ));
                }
            } else {
                return Err(Error::new(
                    arg.span(),
                    "Second argument must be one of the allowed context types",
                ));
            }
        }
        _ => {
            return Err(Error::new(
                handler.sig.ident.span(),
                "Invalid handler arguments. It should be like (`self`, `ctx`, optional arg)",
            ));
        }
    };

    match args_iter.next() {
        Some(FnArg::Typed(type_arg)) => {
            return Ok(Some(type_arg.clone()));
        }
        Some(FnArg::Receiver(arg)) => {
            return Err(Error::new(
                arg.span(),
                "Invalid handler arguments. It should be like (`self`, `ctx`, arg)",
            ));
        }
        None => {
            return Ok(None);
        }
    };
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
