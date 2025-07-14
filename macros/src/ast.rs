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
use syn::{
    parse_quote, Attribute, Error, Expr, ExprLit, FnArg, GenericArgument, Ident, ImplItem,
    ImplItemFn, Item, ItemImpl, ItemTrait, Lit, Meta, Pat, PatType, Path, PathArguments, Result,
    ReturnType, TraitItem, TraitItemFn, Type, Visibility,
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

pub(crate) struct TraitBlockServiceInner {
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) restate_name: String,
    pub(crate) vis: Visibility,
    pub(crate) ident: Ident,
    pub(crate) handlers: Vec<Handler>,
}

impl TraitBlockServiceInner {
    fn parse(service_type: ServiceType, input: ItemTrait) -> Result<Self> {
        let parsed_attrs = input.attrs;
        let vis = input.vis;
        let ident: Ident = input.ident;
        let mut rpcs = Vec::<Handler>::new();
        for item in input.items {
            if let TraitItem::Fn(handler) = item {
                let handler: Handler = Handler::parse(handler)?;
                if handler.is_shared && service_type == ServiceType::Service {
                    return Err(Error::new(
                        handler.ident.span(),
                        "Service handlers cannot be annotated with #[shared]",
                    ));
                }
                rpcs.push(handler);
            }
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

pub(crate) struct ImplBlockServiceInner {
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) restate_name: String,
    pub(crate) vis: Visibility,
    pub(crate) ident: Ident,
    pub(crate) handlers: Vec<Handler>,
    pub(crate) impl_block: ItemImpl,
}

impl ImplBlockServiceInner {
    fn parse(service_type: ServiceType, mut input: ItemImpl) -> Result<Self> {
        let ident = match input.self_ty.as_ref() {
            Type::Path(path) => path.path.segments[0].ident.clone(),
            bad_path => {
                return Err(Error::new(bad_path.span(), "Only on impl blocks"));
            }
        };

        let mut rpcs = Vec::new();
        for item in input.items.iter_mut() {
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

                        handler.attrs = attrs.clone();

                        rpcs.push(Handler {
                            attrs,
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
                    return Err(Error::new(bad_impl_item.span(), "Only on consts and fns"));
                }
            }
        }

        Ok(Self {
            attrs: input.attrs.clone(),
            restate_name: "".to_string(),
            ident,
            vis: Visibility::Inherited,
            handlers: rpcs,
            impl_block: input,
        })
    }
}

pub(crate) enum ServiceInner {
    Trait(TraitBlockServiceInner),
    Impl(ImplBlockServiceInner),
}

impl ServiceInner {
    fn parse(service_type: ServiceType, input: ParseStream) -> Result<Self> {
        let item = input.parse()?;

        match item {
            Item::Trait(trait_block) => Ok(Self::Trait(TraitBlockServiceInner::parse(
                service_type,
                trait_block,
            )?)),
            Item::Impl(impl_block) => Ok(Self::Impl(ImplBlockServiceInner::parse(
                service_type,
                impl_block,
            )?)),
            other => Err(syn::Error::new_spanned(
                other,
                "expected `impl` or `struct`",
            )),
        }
    }
}

pub(crate) struct Handler {
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) is_shared: bool,
    pub(crate) restate_name: String,
    pub(crate) ident: Ident,
    pub(crate) arg: Option<PatType>,
    pub(crate) output_ok: Type,
    pub(crate) output_err: Type,
}

impl Handler {
    fn parse(input: TraitItemFn) -> Result<Self> {
        let parsed_attrs = input.attrs;
        let ident: Ident = input.sig.ident;
        if input.sig.asyncness.is_none() {
            return Err(Error::new(ident.span(), "Handlers must be `async`"));
        }

        let mut args = Vec::new();
        let mut errors = Ok(());
        for arg in &input.sig.inputs {
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
            if args.len() > 1 {
                extend_errors!(
                    errors,
                    Error::new(
                        input.sig.inputs.span(),
                        "Only one input argument is supported"
                    ) // TODO: is this a correct span
                );
                break;
            }
        }
        errors?;

        let return_type: ReturnType = input.sig.output;
        if input.default.is_some() {
            return Err(Error::new(
                ident.span(),
                "Default trait method impl isn't supported",
            ));
        }

        let (ok_ty, err_ty) = match &return_type {
            ReturnType::Default => return Err(Error::new(
                return_type.span(),
                "The return type cannot be empty, only Result or restate_sdk::prelude::HandlerResult is supported as return type",
            )),
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
        let mut restate_name = ident.to_string();
        let mut attrs = vec![];
        for attr in parsed_attrs {
            if is_shared_attr(&attr) {
                is_shared = true;
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
            restate_name,
            ident,
            arg: args.pop().cloned(),
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

fn read_literal_attribute_name(attr: &Attribute) -> Result<Option<String>> {
    attr.meta
        .require_name_value()
        .ok()
        .filter(|val| val.path.require_ident().is_ok_and(|i| i == "name"))
        .map(|val| {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(ref literal),
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

    // TODO: allow the user to have unused context like _:Context in the handler
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
        Some(FnArg::Typed(type_arg)) => Ok(Some(type_arg.clone())),
        Some(FnArg::Receiver(arg)) => Err(Error::new(
            arg.span(),
            "Invalid handler arguments. It should be like (`self`, `ctx`, arg)",
        )),
        None => Ok(None),
    }
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
