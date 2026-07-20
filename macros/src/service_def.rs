// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Implementation of the declarative `service!` / `object!` / `workflow!` item macros.
//!
//! `service!(Greeter: { greet, other })` defines a zero-sized `Greeter` service type whose runtime
//! `Service` impl hardcodes the name→handler `match` over the listed [`macro@crate::handler`]
//! values, plus a `GreeterClient`, from a single declaration — no runtime builder or dispatch map.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Error, Ident, LitStr, Token, braced};

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Service,
    Object,
    Workflow,
}

/// Parsed `Name: { handler, handler, ... }`.
struct ServiceDef {
    name: Ident,
    handlers: Vec<Ident>,
}

impl Parse for ServiceDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;

        let content;
        braced!(content in input);
        let handlers: Punctuated<Ident, Token![,]> =
            content.parse_terminated(Ident::parse, Token![,])?;

        // Reserve the syntax for future extension (service-level `options: { .. }`,
        // per-handler config blocks, an explicit wire-name, ...).
        if !input.is_empty() {
            return Err(Error::new(
                input.span(),
                "only `Name: { handler, ... }` is supported for now; \
                 per-handler config and service options are not yet available in the macro \
                 (use `.options(..)` on the generated type for options)",
            ));
        }

        Ok(ServiceDef {
            name,
            handlers: handlers.into_iter().collect(),
        })
    }
}

pub(crate) fn expand(kind: Kind, input: TokenStream) -> syn::Result<TokenStream> {
    let def: ServiceDef = syn::parse2(input)?;
    let name = &def.name;
    let handlers = &def.handlers;
    crate::names::validate_service_name(&name.to_string(), name.span())?;
    let name_str = LitStr::new(&name.to_string(), name.span());
    let client_ident = format_ident!("{}Client", name);

    let kind_path = match kind {
        Kind::Service => quote!(::restate_sdk::service::macro_support::ServiceKind),
        Kind::Object => quote!(::restate_sdk::service::macro_support::ObjectKind),
        Kind::Workflow => quote!(::restate_sdk::service::macro_support::WorkflowKind),
    };
    let service_type = match kind {
        Kind::Service => quote!(::restate_sdk::discovery::ServiceType::Service),
        Kind::Object => quote!(::restate_sdk::discovery::ServiceType::VirtualObject),
        Kind::Workflow => quote!(::restate_sdk::discovery::ServiceType::Workflow),
    };

    // Server dispatch: a hardcoded `if` per handler, calling straight into its `Handler::handle`.
    // The `<#h as Handler<#kind_path>>` qualification also enforces that every listed handler is
    // of this service's kind.
    let dispatch_arms = handlers.iter().map(|h| {
        quote! {
            if ctx.handler_name() == #h::NAME {
                return <#h as ::restate_sdk::service::macro_support::Handler<#kind_path>>::handle(&#h, ctx);
            }
        }
    });
    // Discovery: each handler's discovery entry, collected for the `Discoverable` impl. The
    // `::<#kind_path, _>` turbofish also enforces that every listed handler is of this kind.
    let handler_discovery = handlers.iter().map(
        |h| quote!(::restate_sdk::service::macro_support::handler_into_discovery::<#kind_path, _>(&#h)),
    );
    // Extensions the handlers declare via `Extension<..>` params, aggregated for build-time validation.
    let handler_required_extensions = handlers.iter().map(
        |h| quote!(v.extend(::restate_sdk::service::macro_support::handler_into_required_extensions::<#kind_path, _>(&#h));),
    );

    // Client to call this service from another handler, typed from each handler's associated
    // input/output types and targeting its real wire name (`#h::NAME`).
    let (key_field, into_client_impl) = match kind {
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
    let client_methods = handlers.iter().map(|h| {
        let target = match kind {
            Kind::Service => quote!(::restate_sdk::context::RequestTarget::service(#name_str, #h::NAME)),
            Kind::Object => {
                quote!(::restate_sdk::context::RequestTarget::object(#name_str, &self.key, #h::NAME))
            }
            Kind::Workflow => {
                quote!(::restate_sdk::context::RequestTarget::workflow(#name_str, &self.key, #h::NAME))
            }
        };
        quote! {
            pub fn #h(
                &self,
                req: <#h as ::restate_sdk::service::macro_support::Handler<#kind_path>>::Input,
            ) -> ::restate_sdk::context::Request<
                'ctx,
                <#h as ::restate_sdk::service::macro_support::Handler<#kind_path>>::Input,
                <#h as ::restate_sdk::service::macro_support::Handler<#kind_path>>::Output,
            > {
                self.ctx.request(#target, req)
            }
        }
    });

    let svc_doc = format!(
        "The `{name}` service definition. Bind it with `Endpoint::builder().bind({name})`, \
         or `{name}.options(..)` to attach options."
    );
    let client_doc = format!("Client to invoke the `{name}` service from another handler.");

    Ok(quote! {
        #[doc = #svc_doc]
        #[derive(::core::clone::Clone, ::core::marker::Copy)]
        pub struct #name;

        // Runtime dispatcher: a hardcoded name→handler `match`, no dispatch map.
        impl ::restate_sdk::service::Service for #name {
            type Future = ::restate_sdk::service::macro_support::ServiceBoxFuture;

            fn handle(
                &self,
                ctx: ::restate_sdk::endpoint::ContextInternal,
            ) -> Self::Future {
                #( #dispatch_arms )*
                ::restate_sdk::service::macro_support::unknown_handler(&ctx)
            }
        }

        impl ::restate_sdk::service::Discoverable for #name {
            fn discover() -> ::restate_sdk::discovery::Service {
                ::restate_sdk::service::macro_support::service_into_discovery(
                    #name_str,
                    #service_type,
                    ::std::vec![ #( #handler_discovery ),* ],
                )
            }

            fn required_extensions(
            ) -> ::std::vec::Vec<::restate_sdk::service::macro_support::RequiredExtension> {
                let mut v = ::std::vec::Vec::new();
                #( #handler_required_extensions )*
                v
            }
        }

        impl #name {
            /// Register a service-scoped dependency (extension), producing a bindable
            /// [`ServiceDefinition`](restate_sdk::service::ServiceDefinition).
            pub fn extension<T: ::std::any::Any + ::core::marker::Send + ::core::marker::Sync>(
                self,
                value: T,
            ) -> ::restate_sdk::service::ServiceDefinition {
                ::restate_sdk::service::IntoServiceDefinition::into_service_definition(self)
                    .extension(value)
            }

            /// Attach [`ServiceOptions`](restate_sdk::endpoint::ServiceOptions), producing a bindable
            /// [`ServiceDefinition`](restate_sdk::service::ServiceDefinition).
            pub fn options(
                self,
                options: ::restate_sdk::endpoint::ServiceOptions,
            ) -> ::restate_sdk::service::ServiceDefinition {
                ::restate_sdk::service::IntoServiceDefinition::into_service_definition(self)
                    .options(options)
            }
        }

        #[doc = #client_doc]
        pub struct #client_ident<'ctx> {
            ctx: &'ctx ::restate_sdk::endpoint::ContextInternal,
            #key_field
        }

        #into_client_impl

        impl<'ctx> #client_ident<'ctx> {
            #( #client_methods )*
        }
    })
}
