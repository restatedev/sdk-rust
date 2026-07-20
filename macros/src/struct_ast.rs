// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Parsing for the struct-based service API: `#[restate_sdk::service]` (and `#[object]`/`#[workflow]`)
//! applied to an inherent `impl` block, with handlers annotated `#[handler]`.

use crate::ast::{ServiceType, extract_handler_result_parameter};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::token::Comma;
use syn::{
    Attribute, Error, Expr, ExprLit, FnArg, Ident, ImplItem, ItemImpl, Lit, Meta, MetaNameValue,
    PatType, Result, ReturnType, Type, Visibility, parse_quote,
};

/// Arguments accepted by the service/object/workflow attribute in the struct API,
/// e.g. `#[restate_sdk::service(name = "Greeter", vis = "pub(crate)")]`.
#[derive(Default)]
pub(crate) struct ServiceArgs {
    pub(crate) name: Option<String>,
    pub(crate) vis: Option<Visibility>,
}

impl Parse for ServiceArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut args = ServiceArgs::default();
        if input.is_empty() {
            return Ok(args);
        }
        let metas = Punctuated::<MetaNameValue, Comma>::parse_terminated(input)?;
        for meta in metas {
            let ident = meta.path.require_ident()?;
            if ident == "name" {
                args.name = Some(parse_str_lit(&meta.value)?);
            } else if ident == "vis" {
                let s = parse_str_lit(&meta.value)?;
                args.vis = Some(syn::parse_str::<Visibility>(&s)?);
            } else {
                return Err(Error::new(
                    ident.span(),
                    "unsupported attribute argument; supported arguments are `name` and `vis`",
                ));
            }
        }
        Ok(args)
    }
}

fn parse_str_lit(expr: &Expr) -> Result<String> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = expr
    {
        Ok(s.value())
    } else {
        Err(Error::new(
            expr.span(),
            "expected a string literal, e.g. `name = \"Greeter\"`",
        ))
    }
}

pub(crate) struct StructService {
    pub(crate) service_ty: ServiceType,
    pub(crate) restate_name: String,
    /// Visibility of the generated client (and impls). Defaults to `pub`.
    pub(crate) vis: Visibility,
    /// The `Self` type of the impl block (e.g. `Greeter` or `Repo<D>`).
    pub(crate) self_ty: Box<Type>,
    /// Last path segment of `self_ty` (e.g. `Greeter`), used to name the client.
    pub(crate) self_ident: Ident,
    /// Generics of the impl block (type params, bounds and where-clause), propagated onto every
    /// generated impl and the client.
    pub(crate) generics: syn::Generics,
    /// The user's impl block with `#[handler]` attributes stripped, re-emitted verbatim.
    pub(crate) stripped_impl: ItemImpl,
    pub(crate) handlers: Vec<StructHandler>,
}

pub(crate) struct StructHandler {
    pub(crate) is_shared: bool,
    pub(crate) restate_name: String,
    pub(crate) ident: Ident,
    pub(crate) arg: Option<PatType>,
    pub(crate) output_ok: Type,
}

impl StructService {
    pub(crate) fn from_impl(
        service_ty: ServiceType,
        args: ServiceArgs,
        mut item_impl: ItemImpl,
    ) -> Result<Self> {
        if let Some((_, path, _)) = &item_impl.trait_ {
            return Err(Error::new_spanned(
                path,
                "the struct-based Restate API must be applied to an inherent impl block \
                 (`impl MyService { .. }`), not a trait impl",
            ));
        }
        for param in &item_impl.generics.params {
            if let syn::GenericParam::Const(c) = param {
                return Err(Error::new_spanned(
                    c,
                    "const generic parameters are not supported on Restate services",
                ));
            }
        }

        let generics = item_impl.generics.clone();
        let self_ty = item_impl.self_ty.clone();
        let self_ident = type_last_ident(&self_ty)?;

        let restate_name = args.name.unwrap_or_else(|| self_ident.to_string());
        let vis = args.vis.unwrap_or_else(|| parse_quote!(pub));

        let mut handlers = Vec::new();
        for item in &mut item_impl.items {
            if let ImplItem::Fn(f) = item
                && let Some(idx) = find_handler_attr(&f.attrs)
            {
                let attr = f.attrs.remove(idx);
                let name_override = read_handler_name(&attr)?;
                handlers.push(parse_handler(service_ty, f, name_override)?);
            }
        }

        if handlers.is_empty() {
            return Err(Error::new_spanned(
                &item_impl,
                "no `#[handler]` methods found; annotate at least one handler method with `#[handler]`",
            ));
        }

        Ok(Self {
            service_ty,
            restate_name,
            vis,
            self_ty,
            self_ident,
            generics,
            stripped_impl: item_impl,
            handlers,
        })
    }
}

/// Index of the `#[handler]` (or `#[restate_sdk::handler]`) attribute in `attrs`, if present.
fn find_handler_attr(attrs: &[Attribute]) -> Option<usize> {
    attrs.iter().position(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "handler")
    })
}

/// Read `name = ".."` from `#[handler(name = "..")]`, if present.
fn read_handler_name(attr: &Attribute) -> Result<Option<String>> {
    match &attr.meta {
        Meta::Path(_) => Ok(None),
        Meta::List(list) => {
            let metas =
                list.parse_args_with(Punctuated::<MetaNameValue, Comma>::parse_terminated)?;
            let mut name = None;
            for m in metas {
                if m.path.require_ident()? == "name" {
                    name = Some(parse_str_lit(&m.value)?);
                } else {
                    return Err(Error::new_spanned(
                        &m.path,
                        "unsupported `#[handler]` argument; the only supported argument is `name`",
                    ));
                }
            }
            Ok(name)
        }
        Meta::NameValue(_) => Err(Error::new_spanned(
            attr,
            "invalid `#[handler]` attribute; use `#[handler]` or `#[handler(name = \"..\")]`",
        )),
    }
}

fn parse_handler(
    service_ty: ServiceType,
    f: &syn::ImplItemFn,
    name_override: Option<String>,
) -> Result<StructHandler> {
    let sig = &f.sig;

    if sig.asyncness.is_none() {
        return Err(Error::new_spanned(
            &sig.ident,
            "Restate handlers must be declared `async fn`",
        ));
    }

    let mut inputs = sig.inputs.iter();

    // First argument: `&self`.
    match inputs.next() {
        Some(FnArg::Receiver(recv)) => {
            if recv.reference.is_none() || recv.mutability.is_some() {
                return Err(Error::new_spanned(
                    recv,
                    "Restate handlers must take `&self`; the service value is shared (behind an Arc) \
                     across concurrent invocations, so use interior mutability for mutable state",
                ));
            }
        }
        _ => {
            return Err(Error::new_spanned(
                &sig.ident,
                "Restate handlers must take `&self` as the first argument",
            ));
        }
    }

    // Second argument: the context.
    let ctx_arg = match inputs.next() {
        Some(FnArg::Typed(pt)) => pt,
        _ => {
            return Err(Error::new_spanned(
                &sig.ident,
                "Restate handlers must take a context as the second argument \
                 (Context, ObjectContext, SharedObjectContext, WorkflowContext or SharedWorkflowContext)",
            ));
        }
    };
    let is_shared = validate_context(service_ty, &ctx_arg.ty)?;

    // Optional third argument: the single input. The parameter pattern (which may destructure,
    // e.g. `Json(x): Json<T>`) stays in the user's impl untouched; we only read its type.
    let mut arg = None;
    for extra in inputs {
        match extra {
            FnArg::Typed(pt) => {
                if arg.is_some() {
                    return Err(Error::new_spanned(
                        extra,
                        "Restate handlers support at most one input argument (after the context)",
                    ));
                }
                arg = Some(pt.clone());
            }
            FnArg::Receiver(recv) => {
                return Err(Error::new_spanned(recv, "unexpected `self` argument"));
            }
        }
    }

    // Return type.
    let (output_ok, _output_err) = match &sig.output {
        ReturnType::Default => {
            return Err(Error::new_spanned(
                &sig.ident,
                "handler return type must be `Result<T, E>` or `restate_sdk::prelude::HandlerResult<T>`",
            ));
        }
        ReturnType::Type(_, ty) => extract_handler_result_parameter(ty).ok_or_else(|| {
            Error::new_spanned(
                ty,
                "handler return type must be `Result<T, E>` or `restate_sdk::prelude::HandlerResult<T>`",
            )
        })?,
    };

    Ok(StructHandler {
        is_shared,
        restate_name: name_override.unwrap_or_else(|| sig.ident.to_string()),
        ident: sig.ident.clone(),
        arg,
        output_ok,
    })
}

/// Validate that the context type matches the declared service kind, returning whether it is a
/// shared (read-only) context.
fn validate_context(service_ty: ServiceType, ty: &Type) -> Result<bool> {
    let ident = type_last_ident(ty)?;
    let (kind, shared) = match ident.to_string().as_str() {
        "Context" => (ServiceType::Service, false),
        "ObjectContext" => (ServiceType::Object, false),
        "SharedObjectContext" => (ServiceType::Object, true),
        "WorkflowContext" => (ServiceType::Workflow, false),
        "SharedWorkflowContext" => (ServiceType::Workflow, true),
        other => {
            return Err(Error::new_spanned(
                ty,
                format!(
                    "unrecognized context type `{other}`; expected one of Context, ObjectContext, \
                     SharedObjectContext, WorkflowContext, SharedWorkflowContext"
                ),
            ));
        }
    };
    if kind != service_ty {
        return Err(Error::new_spanned(
            ty,
            format!(
                "context type `{ident}` does not match the `#[{}]` service kind; expected {}",
                macro_name(service_ty),
                expected_contexts(service_ty),
            ),
        ));
    }
    Ok(shared)
}

fn macro_name(service_ty: ServiceType) -> &'static str {
    match service_ty {
        ServiceType::Service => "service",
        ServiceType::Object => "object",
        ServiceType::Workflow => "workflow",
    }
}

fn expected_contexts(service_ty: ServiceType) -> &'static str {
    match service_ty {
        ServiceType::Service => "Context",
        ServiceType::Object => "ObjectContext or SharedObjectContext",
        ServiceType::Workflow => "WorkflowContext or SharedWorkflowContext",
    }
}

/// The last path segment ident of a type path (e.g. `Greeter` for `Greeter` or `Context` for
/// `Context<'_>`).
fn type_last_ident(ty: &Type) -> Result<Ident> {
    match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|seg| seg.ident.clone())
            .ok_or_else(|| Error::new_spanned(ty, "expected a named type")),
        _ => Err(Error::new_spanned(ty, "expected a named type")),
    }
}
