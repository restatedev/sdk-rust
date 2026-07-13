// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Implementation of the `restate_sdk::interface!` function-like macro.
//!
//! Generates, from a small DSL, a typed client to call a service AND a conformance-checked server
//! builder. The client can live in a crate shared with callers, without depending on the impl.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{braced, parenthesized, parse_quote, Error, Ident, LitStr, Token, Type};

#[derive(Clone, Copy)]
enum Kind {
    Service,
    Object,
    Workflow,
}

struct Interface {
    kind: Kind,
    name: Ident,
    restate_name: String,
    handlers: Vec<IfaceHandler>,
}

struct IfaceHandler {
    name: Ident,
    restate_name: String,
    input: Option<Type>,
    output: Type,
}

impl Parse for Interface {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let kw: Ident = input.parse()?;
        let kind = match kw.to_string().as_str() {
            "service" => Kind::Service,
            "object" => Kind::Object,
            "workflow" => Kind::Workflow,
            _ => {
                return Err(Error::new(
                    kw.span(),
                    "expected `service`, `object` or `workflow`",
                ));
            }
        };
        let name: Ident = input.parse()?;
        let restate_name = name.to_string();

        let content;
        braced!(content in input);
        let mut handlers = Vec::new();
        while !content.is_empty() {
            handlers.push(content.parse()?);
        }

        Ok(Interface {
            kind,
            name,
            restate_name,
            handlers,
        })
    }
}

impl Parse for IfaceHandler {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Optional `#[name = "restateName"]` to decouple the Rust method name from the wire name.
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let name: Ident = input.parse()?;
        let mut restate_name = name.to_string();
        for attr in attrs {
            if attr.path().is_ident("name") {
                if let syn::Meta::NameValue(nv) = &attr.meta
                    && let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = &nv.value
                {
                    restate_name = s.value();
                } else {
                    return Err(Error::new_spanned(
                        &attr.meta,
                        "expected `#[name = \"...\"]`",
                    ));
                }
            } else {
                return Err(Error::new_spanned(attr, "unsupported attribute; only `#[name = \"...\"]` is allowed"));
            }
        }

        let args;
        parenthesized!(args in input);
        let handler_input = if args.is_empty() {
            None
        } else {
            let ty: Type = args.parse()?;
            if !args.is_empty() {
                return Err(Error::new(
                    args.span(),
                    "a handler accepts at most one input type",
                ));
            }
            Some(ty)
        };

        input.parse::<Token![->]>()?;
        let output: Type = input.parse()?;
        input.parse::<Token![;]>()?;

        Ok(IfaceHandler {
            name,
            restate_name,
            input: handler_input,
            output,
        })
    }
}

pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let iface: Interface = syn::parse2(input)?;
    let client = generate_client(&iface);
    let server = generate_server(&iface);
    Ok(quote! {
        #client
        #server
    })
}

fn generate_client(iface: &Interface) -> TokenStream {
    let client_ident = format_ident!("{}Client", iface.name);
    let service_lit = LitStr::new(&iface.restate_name, iface.name.span());

    let (key_field, into_client_impl) = match iface.kind {
        Kind::Service => (
            quote!(),
            quote! {
                impl<'ctx> ::restate_sdk::context::IntoServiceClient<'ctx> for #client_ident<'ctx> {
                    fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal) -> Self {
                        Self { ctx }
                    }
                }
            },
        ),
        Kind::Object => (
            quote!(key: String,),
            quote! {
                impl<'ctx> ::restate_sdk::context::IntoObjectClient<'ctx> for #client_ident<'ctx> {
                    fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal, key: String) -> Self {
                        Self { ctx, key }
                    }
                }
            },
        ),
        Kind::Workflow => (
            quote!(key: String,),
            quote! {
                impl<'ctx> ::restate_sdk::context::IntoWorkflowClient<'ctx> for #client_ident<'ctx> {
                    fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal, key: String) -> Self {
                        Self { ctx, key }
                    }
                }
            },
        ),
    };

    let methods = iface.handlers.iter().map(|h| {
        let method = &h.name;
        let handler_lit = LitStr::new(&h.restate_name, h.name.span());
        let (arg, arg_ty, input_expr) = match &h.input {
            Some(ty) => (quote!(, req: #ty), quote!(#ty), quote!(req)),
            None => (quote!(), quote!(()), quote!(())),
        };
        let res_ty = &h.output;
        let target = match iface.kind {
            Kind::Service => quote! {
                ::restate_sdk::context::RequestTarget::service(#service_lit, #handler_lit)
            },
            Kind::Object => quote! {
                ::restate_sdk::context::RequestTarget::object(#service_lit, &self.key, #handler_lit)
            },
            Kind::Workflow => quote! {
                ::restate_sdk::context::RequestTarget::workflow(#service_lit, &self.key, #handler_lit)
            },
        };
        quote! {
            pub fn #method(&self #arg) -> ::restate_sdk::context::Request<'ctx, #arg_ty, #res_ty> {
                self.ctx.request(#target, #input_expr)
            }
        }
    });

    let doc = format!("Client to invoke the `{}` service.", iface.restate_name);
    quote! {
        #[doc = #doc]
        pub struct #client_ident<'ctx> {
            ctx: &'ctx ::restate_sdk::endpoint::ContextInternal,
            #key_field
        }

        #into_client_impl

        impl<'ctx> #client_ident<'ctx> {
            #( #methods )*
        }
    }
}

fn generate_server(iface: &Interface) -> TokenStream {
    let name = &iface.name;
    let handlers_struct = format_ident!("{}Handlers", iface.name);
    let service_lit = LitStr::new(&iface.restate_name, iface.name.span());

    let kind_path = match iface.kind {
        Kind::Service => quote!(::restate_sdk::service::ServiceKind),
        Kind::Object => quote!(::restate_sdk::service::ObjectKind),
        Kind::Workflow => quote!(::restate_sdk::service::WorkflowKind),
    };
    let builder_fn = match iface.kind {
        Kind::Service => quote!(::restate_sdk::service::service),
        Kind::Object => quote!(::restate_sdk::service::object),
        Kind::Workflow => quote!(::restate_sdk::service::workflow),
    };

    let generics: Vec<Ident> = (0..iface.handlers.len())
        .map(|i| format_ident!("H{}", i))
        .collect();

    let fields = iface.handlers.iter().zip(&generics).map(|(h, g)| {
        let field = &h.name;
        quote!(pub #field: #g)
    });

    let bounds = iface.handlers.iter().zip(&generics).map(|(h, g)| {
        let input: Type = h.input.clone().unwrap_or_else(|| parse_quote!(()));
        let output = &h.output;
        quote! {
            #g: ::restate_sdk::service::TypedHandler<#kind_path, Input = #input, Output = #output>
        }
    });

    let add_handlers = iface.handlers.iter().map(|h| {
        let field = &h.name;
        let handler_lit = LitStr::new(&h.restate_name, h.name.span());
        // Register under the interface's declared name so client and server stay in sync.
        quote! {
            .handler(::restate_sdk::service::Named { handler: handlers.#field, name: #handler_lit })
        }
    });

    let handlers_doc = format!("Handlers required to serve the `{}` service.", iface.restate_name);
    let server_doc = format!(
        "Build a conformance-checked [`ServiceDefinition`] for the `{}` service.",
        iface.restate_name
    );

    quote! {
        #[doc = #handlers_doc]
        pub struct #handlers_struct<#(#generics),*> {
            #( #fields ),*
        }

        #[doc = #server_doc]
        pub struct #name;

        impl #name {
            #[doc = #server_doc]
            pub fn from_handlers<#(#generics),*>(
                handlers: #handlers_struct<#(#generics),*>,
            ) -> ::restate_sdk::service::ServiceDefinition
            where
                #( #bounds ),*
            {
                #builder_fn(#service_lit)
                    #( #add_handlers )*
                    .build()
            }
        }
    }
}
