// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Implementation of the `#[restate_sdk::handler]` attribute macro.
//!
//! Turns a free `async fn` into a by-value handler value implementing
//! `restate_sdk::service::Handler` (and `TypedHandler`). The service kind and whether the handler
//! is shared are inferred from the context-type of the first parameter.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    Error, FnArg, GenericArgument, ItemFn, Lit, Meta, PatType, PathArguments, ReturnType, Token,
    Type,
};

/// The service kind + shared-ness, inferred from the first parameter's context type.
enum CtxKind {
    Service,
    ObjectExclusive,
    ObjectShared,
    WorkflowRun,
    WorkflowShared,
}

impl CtxKind {
    fn detect(ty: &Type) -> Option<Self> {
        let path = match ty {
            Type::Path(tp) => &tp.path,
            _ => return None,
        };
        let ident = &path.segments.last()?.ident;
        Some(match ident.to_string().as_str() {
            "Context" => CtxKind::Service,
            "ObjectContext" => CtxKind::ObjectExclusive,
            "SharedObjectContext" => CtxKind::ObjectShared,
            "WorkflowContext" => CtxKind::WorkflowRun,
            "SharedWorkflowContext" => CtxKind::WorkflowShared,
            _ => return None,
        })
    }

    /// The marker type path for `Handler<Kind>`.
    fn kind_path(&self) -> TokenStream {
        match self {
            CtxKind::Service => quote!(::restate_sdk::service::ServiceKind),
            CtxKind::ObjectExclusive | CtxKind::ObjectShared => {
                quote!(::restate_sdk::service::ObjectKind)
            }
            CtxKind::WorkflowRun | CtxKind::WorkflowShared => {
                quote!(::restate_sdk::service::WorkflowKind)
            }
        }
    }

    /// The discovery `HandlerType`, following the same defaulting rules as the discovery manifest.
    fn discovery_ty(&self) -> TokenStream {
        match self {
            CtxKind::Service | CtxKind::ObjectExclusive => quote!(::core::option::Option::None),
            CtxKind::ObjectShared | CtxKind::WorkflowShared => {
                quote!(::core::option::Option::Some(
                    ::restate_sdk::discovery::HandlerType::Shared
                ))
            }
            CtxKind::WorkflowRun => {
                quote!(::core::option::Option::Some(
                    ::restate_sdk::discovery::HandlerType::Workflow
                ))
            }
        }
    }
}

/// Parsed `#[handler(...)]` attribute arguments.
struct HandlerAttrs {
    name: Option<String>,
    lazy_state: bool,
}

impl HandlerAttrs {
    fn parse(attr: TokenStream) -> syn::Result<Self> {
        let mut name = None;
        let mut lazy_state = false;
        if attr.is_empty() {
            return Ok(Self { name, lazy_state });
        }
        let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(attr)?;
        for meta in metas {
            match meta {
                Meta::NameValue(nv) if nv.path.is_ident("name") => {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: Lit::Str(s), ..
                    }) = nv.value
                    {
                        name = Some(s.value());
                    } else {
                        return Err(Error::new(
                            nv.value.span(),
                            "expected a string literal for `name`",
                        ));
                    }
                }
                Meta::Path(p) if p.is_ident("lazy_state") => lazy_state = true,
                other => {
                    return Err(Error::new(
                        other.span(),
                        "unsupported #[handler] argument; expected `name = \"...\"` or `lazy_state`",
                    ));
                }
            }
        }
        Ok(Self { name, lazy_state })
    }
}

/// Extract the `Ok` type from a `Result<T, E>` / `HandlerResult<T>` return type.
fn extract_ok_type(output: &ReturnType) -> syn::Result<Type> {
    let ty = match output {
        ReturnType::Default => {
            return Err(Error::new(
                output.span(),
                "handler must return a Result or HandlerResult",
            ));
        }
        ReturnType::Type(_, ty) => ty,
    };
    let err = || {
        Error::new(
            ty.span(),
            "handler return type must be Result<T, E> or HandlerResult<T>",
        )
    };
    let path = match &**ty {
        Type::Path(tp) => &tp.path,
        _ => return Err(err()),
    };
    let seg = path.segments.last().ok_or_else(err)?;
    let args = match &seg.arguments {
        PathArguments::AngleBracketed(a) => a,
        _ => return Err(err()),
    };
    let is_handler_result = seg.ident == "HandlerResult" && args.args.len() == 1;
    let is_result = seg.ident == "Result" && args.args.len() == 2;
    if (is_handler_result || is_result)
        && let Some(GenericArgument::Type(t)) = args.args.first()
    {
        return Ok(t.clone());
    }
    Err(err())
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let attrs = HandlerAttrs::parse(attr)?;
    let func: ItemFn = syn::parse2(item)?;

    if func.sig.asyncness.is_none() {
        return Err(Error::new(
            func.sig.fn_token.span(),
            "handler function must be `async`",
        ));
    }
    if !func.sig.generics.params.is_empty() {
        return Err(Error::new(
            func.sig.generics.span(),
            "handler functions cannot have generic parameters",
        ));
    }

    // Parse arguments: first is the context, an optional second is the (single) input.
    let mut iter = func.sig.inputs.iter();
    let ctx_arg = match iter.next() {
        Some(FnArg::Typed(pt)) => pt,
        Some(FnArg::Receiver(r)) => {
            return Err(Error::new(
                r.span(),
                "handler must be a free function and cannot take `self`",
            ));
        }
        None => {
            return Err(Error::new(
                func.sig.span(),
                "handler must take a Restate context as its first parameter",
            ));
        }
    };
    let input_arg: Option<&PatType> = match iter.next() {
        Some(FnArg::Typed(pt)) => Some(pt),
        Some(FnArg::Receiver(r)) => {
            return Err(Error::new(r.span(), "handler cannot take `self`"));
        }
        None => None,
    };
    if let Some(extra) = iter.next() {
        return Err(Error::new(
            extra.span(),
            "handler supports at most one input parameter besides the context",
        ));
    }

    let ctx_kind = CtxKind::detect(&ctx_arg.ty).ok_or_else(|| {
        Error::new(
            ctx_arg.ty.span(),
            "the first parameter must be a Restate context type (Context, ObjectContext, \
             SharedObjectContext, WorkflowContext or SharedWorkflowContext)",
        )
    })?;
    let kind_path = ctx_kind.kind_path();
    let discovery_ty = ctx_kind.discovery_ty();

    let ok_ty = extract_ok_type(&func.sig.output)?;
    let input_ty: Option<Type> = input_arg.map(|pt| (*pt.ty).clone());

    // Discovery schemas.
    let input_schema = match &input_ty {
        Some(ty) => quote!(::restate_sdk::discovery::InputPayload::from_metadata::<#ty>()),
        None => quote!(::restate_sdk::discovery::InputPayload::empty()),
    };
    let output_schema = match &ok_ty {
        Type::Tuple(t) if t.elems.is_empty() => {
            quote!(::restate_sdk::discovery::OutputPayload::empty())
        }
        _ => quote!(::restate_sdk::discovery::OutputPayload::from_metadata::<#ok_ty>()),
    };
    let input_assoc = match &input_ty {
        Some(ty) => quote!(#ty),
        None => quote!(()),
    };

    let ident = &func.sig.ident;

    // Dispatch shim: deserialize input, build the borrowing context, run the body.
    let get_input_and_call = match &input_ty {
        Some(ty) => quote! {
            let (input, metadata) = ctx.input::<#ty>().await;
            let fut = #ident::call((&ctx, metadata).into(), input);
        },
        None => quote! {
            let (_, metadata) = ctx.input::<()>().await;
            let fut = #ident::call((&ctx, metadata).into());
        },
    };

    let restate_name = attrs.name.unwrap_or_else(|| func.sig.ident.to_string());
    let name_lit = syn::LitStr::new(&restate_name, func.sig.ident.span());
    let lazy_state = if attrs.lazy_state {
        quote!(::core::option::Option::Some(true))
    } else {
        quote!(::core::option::Option::None)
    };

    let vis = &func.vis;
    let fn_attrs = &func.attrs;
    let inputs = &func.sig.inputs;
    let output = &func.sig.output;
    let block = &func.block;

    Ok(quote! {
        #(#fn_attrs)*
        #[derive(::core::clone::Clone, ::core::marker::Copy)]
        #[allow(non_camel_case_types)]
        #vis struct #ident;

        impl #ident {
            /// The Restate wire name of this handler (respects `#[handler(name = "...")]`).
            #vis const NAME: &'static str = #name_lit;

            /// Invoke the handler body directly (e.g. for unit tests).
            #vis async fn call(#inputs) #output #block
        }

        impl ::restate_sdk::service::Handler<#kind_path> for #ident {
            fn meta(&self) -> ::restate_sdk::service::HandlerMeta {
                ::restate_sdk::service::HandlerMeta {
                    name: ::std::borrow::Cow::Borrowed(#name_lit),
                    ty: #discovery_ty,
                    input: #input_schema,
                    output: #output_schema,
                    enable_lazy_state: #lazy_state,
                }
            }

            fn handle(
                &self,
                ctx: ::restate_sdk::endpoint::ContextInternal,
            ) -> ::restate_sdk::service::ServiceBoxFuture {
                ::std::boxed::Box::pin(async move {
                    #get_input_and_call
                    let res = fut.await.map_err(::restate_sdk::errors::HandlerError::from);
                    ctx.handle_handler_result(res);
                    ctx.end();
                    ::core::result::Result::Ok(())
                })
            }
        }

        impl ::restate_sdk::service::TypedHandler<#kind_path> for #ident {
            type Input = #input_assoc;
            type Output = #ok_ty;
        }
    })
}
