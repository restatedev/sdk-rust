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
//! `restate_sdk::service::macro_support::Handler`. The service kind and whether the handler is
//! shared are inferred from the context-type of the first parameter.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
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
            CtxKind::Service => quote!(::restate_sdk::service::macro_support::ServiceKind),
            CtxKind::ObjectExclusive | CtxKind::ObjectShared => {
                quote!(::restate_sdk::service::macro_support::ObjectKind)
            }
            CtxKind::WorkflowRun | CtxKind::WorkflowShared => {
                quote!(::restate_sdk::service::macro_support::WorkflowKind)
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

/// Recognize an `Extension<T>` parameter type, returning the inner type `T` and whether it was
/// written as a reference (`Extension<&T>`, which is rejected — extensions are cloned out, like
/// axum's `State`/`Extension`).
fn parse_extension(ty: &Type) -> Option<(Type, bool)> {
    let tp = match ty {
        Type::Path(tp) => tp,
        _ => return None,
    };
    let seg = tp.path.segments.last()?;
    if seg.ident != "Extension" {
        return None;
    }
    let args = match &seg.arguments {
        PathArguments::AngleBracketed(a) => a,
        _ => return None,
    };
    let inner = match args.args.first()? {
        GenericArgument::Type(t) => t,
        _ => return None,
    };
    match inner {
        Type::Reference(r) => Some(((*r.elem).clone(), true)),
        other => Some((other.clone(), false)),
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

    // Parse arguments: first is the context; the rest are, in any order, `Extension<..>` extractors
    // and at most one input parameter.
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

    // A slot in the argument list passed to `call`, so the generated dispatch shim can reconstruct
    // the handler's parameters in their original order.
    enum ArgSlot {
        /// An `Extension<T>` extractor, holding the looked-up type `T`.
        Extension(Box<Type>),
        /// The (single) deserialized input parameter.
        Input,
    }
    let mut slots: Vec<ArgSlot> = Vec::new();
    let mut input_arg: Option<&PatType> = None;
    for arg in iter {
        let pt = match arg {
            FnArg::Typed(pt) => pt,
            FnArg::Receiver(r) => return Err(Error::new(r.span(), "handler cannot take `self`")),
        };
        if let Some((lookup, is_ref)) = parse_extension(&pt.ty) {
            if is_ref {
                return Err(Error::new(
                    pt.ty.span(),
                    "borrowed extensions (`Extension<&T>`) are not supported; use `Extension<T>` \
                     with `T: Clone` (wrap shared or expensive dependencies in `Arc`)",
                ));
            }
            slots.push(ArgSlot::Extension(Box::new(lookup)));
        } else if input_arg.is_none() {
            input_arg = Some(pt);
            slots.push(ArgSlot::Input);
        } else {
            return Err(Error::new(
                pt.span(),
                "handler supports at most one input parameter besides the context and \
                 `Extension<..>` parameters",
            ));
        }
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

    // Associated `Input`/`Output` types: the real types, or `()` for no input/output. `()` impls
    // `PayloadMetadata` by rendering as an empty discovery payload, so discovery derives from these.
    let input_assoc = match &input_ty {
        Some(ty) => quote!(#ty),
        None => quote!(()),
    };

    let ident = &func.sig.ident;
    let ctx_ty = &ctx_arg.ty;

    // Dispatch shim: deserialize input, build the context, clone the extensions out of it, run the
    // body. `call` receives the parameters in their original order; each `Extension` is cloned into
    // a local before the context is moved in as the first argument.
    let mut ext_locals = Vec::new();
    let mut call_args = vec![quote!(restate_ctx)];
    let mut required_ext: Vec<Type> = Vec::new();
    for slot in &slots {
        match slot {
            ArgSlot::Input => call_args.push(quote!(input)),
            ArgSlot::Extension(lookup) => {
                let lookup = &**lookup;
                let local = format_ident!("__restate_ext_{}", required_ext.len());
                ext_locals.push(quote! {
                    let #local = ::restate_sdk::context::Extension(::core::clone::Clone::clone(
                        ::restate_sdk::context::ContextExtensions::extension::<#lookup>(&restate_ctx),
                    ));
                });
                call_args.push(quote!(#local));
                required_ext.push(lookup.clone());
            }
        }
    }
    let input_bind = match &input_ty {
        Some(ty) => quote!(let (input, metadata) = ctx.input::<#ty>().await;),
        None => quote!(let (_, metadata) = ctx.input::<()>().await;),
    };
    let get_input_and_call = quote! {
        #input_bind
        let restate_ctx: #ctx_ty = ::core::convert::Into::into((&ctx, metadata));
        #( #ext_locals )*
        let fut = #ident::call( #( #call_args ),* );
    };

    // Only override the default (empty) `required_extensions` when the handler declares some.
    let required_ext_method = if required_ext.is_empty() {
        quote!()
    } else {
        quote! {
            fn required_extensions(
                &self,
            ) -> ::std::vec::Vec<(::std::any::TypeId, &'static str)> {
                ::std::vec![
                    #( (
                        ::std::any::TypeId::of::<#required_ext>(),
                        ::std::any::type_name::<#required_ext>()
                    ) ),*
                ]
            }
        }
    };

    let restate_name = attrs.name.unwrap_or_else(|| func.sig.ident.to_string());
    let name_lit = syn::LitStr::new(&restate_name, func.sig.ident.span());
    crate::names::validate_handler_name(&restate_name, name_lit.span())?;
    let options_expr = if attrs.lazy_state {
        quote!(::restate_sdk::endpoint::HandlerOptions::default().enable_lazy_state(true))
    } else {
        quote!(::restate_sdk::endpoint::HandlerOptions::default())
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

            /// The handler body as a plain async fn: reusable from other handlers and invoked by
            /// the dispatch shim below.
            #vis async fn call(#inputs) #output #block
        }

        impl ::restate_sdk::service::macro_support::Handler<#kind_path> for #ident {
            type Input = #input_assoc;
            type Output = #ok_ty;

            fn name(&self) -> ::std::borrow::Cow<'static, str> {
                ::std::borrow::Cow::Borrowed(#name_lit)
            }

            fn ty(&self) -> ::core::option::Option<::restate_sdk::discovery::HandlerType> {
                #discovery_ty
            }

            fn options(&self) -> ::restate_sdk::endpoint::HandlerOptions {
                #options_expr
            }

            #required_ext_method

            fn handle(
                &self,
                ctx: ::restate_sdk::endpoint::ContextInternal,
            ) -> ::restate_sdk::service::macro_support::ServiceBoxFuture {
                ::std::boxed::Box::pin(async move {
                    #get_input_and_call
                    let res = fut.await.map_err(::restate_sdk::errors::HandlerError::from);
                    ctx.handle_handler_result(res);
                    ctx.end();
                    ::core::result::Result::Ok(())
                })
            }
        }
    })
}
