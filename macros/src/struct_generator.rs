// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Code generation for the struct-based service API. Lowers an annotated inherent `impl` block onto
//! the same `Service`/`Discoverable`/`IntoServiceDefinition` runtime traits used by the trait API.

use crate::ast::ServiceType;
use crate::struct_ast::{StructHandler, StructService};
use proc_macro2::{Literal, TokenStream as TokenStream2};
use quote::quote;
use syn::PatType;

pub(crate) fn generate(svc: &StructService) -> TokenStream2 {
    let stripped_impl = &svc.stripped_impl;
    let dispatcher = dispatcher_block(svc);
    let client = client(svc);

    quote! {
        #stripped_impl
        #dispatcher
        #client
    }
}

/// The hidden dispatcher wrapper + its `Service`/`Discoverable` impls + the `IntoServiceDefinition`
/// impl for the user type, all scoped inside a `const _` block to avoid polluting the namespace.
fn dispatcher_block(svc: &StructService) -> TokenStream2 {
    let self_ty = &svc.self_ty;
    let (impl_generics, ty_generics, where_clause) = svc.generics.split_for_impl();
    let match_arms = svc
        .handlers
        .iter()
        .map(|handler| dispatch_arm(self_ty, handler));
    let discovery = discovery(svc);

    quote! {
        const _: () = {
            struct RestateServe #impl_generics #where_clause {
                service: ::std::sync::Arc<#self_ty>,
            }

            impl #impl_generics ::restate_sdk::service::Service for RestateServe #ty_generics #where_clause {
                type Future = ::restate_sdk::service::macro_support::ServiceBoxFuture;

                fn handle(&self, ctx: ::restate_sdk::endpoint::ContextInternal) -> Self::Future {
                    let service_clone = ::std::sync::Arc::clone(&self.service);
                    ::std::boxed::Box::pin(async move {
                        match ctx.handler_name() {
                            #( #match_arms ),*
                            _ => {
                                return Err(::restate_sdk::endpoint::Error::unknown_handler(
                                    ctx.service_name(),
                                    ctx.handler_name(),
                                ))
                            }
                        }
                    })
                }
            }

            impl #impl_generics ::restate_sdk::service::IntoServiceDefinition for #self_ty #where_clause {
                fn into_service_definition(self) -> ::restate_sdk::service::ServiceDefinition {
                    let discovery = <#self_ty as ::restate_sdk::service::Discoverable>::discover();
                    ::restate_sdk::service::macro_support::service_definition(
                        RestateServe {
                            service: ::std::sync::Arc::new(self),
                        },
                        discovery,
                    )
                }
            }
        };

        #discovery
    }
}

fn dispatch_arm(self_ty: &syn::Type, handler: &StructHandler) -> TokenStream2 {
    let handler_ident = &handler.ident;
    let handler_literal = Literal::string(&handler.restate_name);

    let get_input_and_call = if handler.arg.is_some() {
        quote! {
            let (input, metadata) = ctx.input().await;
            let fut = <#self_ty>::#handler_ident(&service_clone, (&ctx, metadata).into(), input);
        }
    } else {
        quote! {
            let (_, metadata) = ctx.input::<()>().await;
            let fut = <#self_ty>::#handler_ident(&service_clone, (&ctx, metadata).into());
        }
    };

    quote! {
        #handler_literal => {
            #get_input_and_call
            let res = fut.await.map_err(::restate_sdk::errors::HandlerError::from);
            ctx.handle_handler_result(res);
            ctx.end();
            Ok(())
        }
    }
}

fn discovery(svc: &StructService) -> TokenStream2 {
    let self_ty = &svc.self_ty;
    let (impl_generics, _ty_generics, where_clause) = svc.generics.split_for_impl();
    let service_literal = Literal::string(&svc.restate_name);

    let service_ty_token = match svc.service_ty {
        ServiceType::Service => quote! { ::restate_sdk::discovery::ServiceType::Service },
        ServiceType::Object => quote! { ::restate_sdk::discovery::ServiceType::VirtualObject },
        ServiceType::Workflow => quote! { ::restate_sdk::discovery::ServiceType::Workflow },
    };

    let handlers = svc.handlers.iter().map(|handler| {
        let handler_literal = Literal::string(&handler.restate_name);

        let handler_ty = if handler.is_shared {
            quote! { Some(::restate_sdk::discovery::HandlerType::Shared) }
        } else if svc.service_ty == ServiceType::Workflow {
            quote! { Some(::restate_sdk::discovery::HandlerType::Workflow) }
        } else {
            quote! { None }
        };

        let input_schema = match &handler.arg {
            Some(PatType { ty, .. }) => quote! {
                Some(::restate_sdk::discovery::InputPayload::from_metadata::<#ty>())
            },
            None => quote! {
                Some(::restate_sdk::discovery::InputPayload::empty())
            },
        };

        let output_ty = &handler.output_ok;
        let output_schema = match output_ty {
            syn::Type::Tuple(tuple) if tuple.elems.is_empty() => quote! {
                Some(::restate_sdk::discovery::OutputPayload::empty())
            },
            _ => quote! {
                Some(::restate_sdk::discovery::OutputPayload::from_metadata::<#output_ty>())
            },
        };

        quote! {
            ::restate_sdk::discovery::Handler {
                name: ::restate_sdk::discovery::HandlerName::try_from(#handler_literal).expect("Handler name valid"),
                input: #input_schema,
                output: #output_schema,
                ty: #handler_ty,
                documentation: None,
                metadata: Default::default(),
                abort_timeout: None,
                inactivity_timeout: None,
                journal_retention: None,
                idempotency_retention: None,
                workflow_completion_retention: None,
                enable_lazy_state: None,
                ingress_private: None,
                retry_policy_initial_interval: None,
                retry_policy_max_interval: None,
                retry_policy_max_attempts: None,
                retry_policy_exponentiation_factor: None,
                retry_policy_on_max_attempts: None,
            }
        }
    });

    quote! {
        impl #impl_generics ::restate_sdk::service::Discoverable for #self_ty #where_clause {
            fn discover() -> ::restate_sdk::discovery::Service {
                ::restate_sdk::discovery::Service {
                    ty: #service_ty_token,
                    name: ::restate_sdk::discovery::ServiceName::try_from(#service_literal.to_string())
                        .expect("Service name valid"),
                    handlers: vec![#( #handlers ),*],
                    documentation: None,
                    metadata: Default::default(),
                    abort_timeout: None,
                    inactivity_timeout: None,
                    journal_retention: None,
                    idempotency_retention: None,
                    enable_lazy_state: None,
                    ingress_private: None,
                    retry_policy_initial_interval: None,
                    retry_policy_max_interval: None,
                    retry_policy_max_attempts: None,
                    retry_policy_exponentiation_factor: None,
                    retry_policy_on_max_attempts: None,
                }
            }
        }
    }
}

/// The `XClient` struct + its `IntoServiceClient`/`IntoObjectClient`/`IntoWorkflowClient` impl +
/// per-handler request methods. Identical in shape to the trait-API client, but carrying the
/// service's generics (so generic services get a generic client).
fn client(svc: &StructService) -> TokenStream2 {
    let vis = &svc.vis;
    let client_ident = quote::format_ident!("{}Client", svc.self_ident);
    let service_literal = Literal::string(&svc.restate_name);

    // Client generics = `'ctx` + the service's generics.
    let mut client_generics = svc.generics.clone();
    client_generics.params.insert(0, syn::parse_quote!('ctx));
    let (client_impl_generics, client_ty_generics, client_where) = client_generics.split_for_impl();

    // A `PhantomData` marker so the service's generic params are considered "used" on the client
    // even when they don't appear in any handler input/output type.
    let marker_types: Vec<TokenStream2> = svc
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(t) => {
                let id = &t.ident;
                Some(quote! { fn() -> #id })
            }
            syn::GenericParam::Lifetime(l) => {
                let lt = &l.lifetime;
                Some(quote! { & #lt () })
            }
            syn::GenericParam::Const(_) => None,
        })
        .collect();
    let (marker_field, marker_init) = if marker_types.is_empty() {
        (quote! {}, quote! {})
    } else {
        (
            quote! { __restate_marker: ::core::marker::PhantomData<(#(#marker_types,)*)>, },
            quote! { __restate_marker: ::core::marker::PhantomData, },
        )
    };

    let key_field = match svc.service_ty {
        ServiceType::Service => quote! {},
        ServiceType::Object | ServiceType::Workflow => quote! { key: String, },
    };

    let into_client_impl = match svc.service_ty {
        ServiceType::Service => quote! {
            impl #client_impl_generics ::restate_sdk::context::IntoServiceClient<'ctx> for #client_ident #client_ty_generics #client_where {
                fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal) -> Self {
                    Self { ctx, #marker_init }
                }
            }
        },
        ServiceType::Object => quote! {
            impl #client_impl_generics ::restate_sdk::context::IntoObjectClient<'ctx> for #client_ident #client_ty_generics #client_where {
                fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal, key: String) -> Self {
                    Self { ctx, key, #marker_init }
                }
            }
        },
        ServiceType::Workflow => quote! {
            impl #client_impl_generics ::restate_sdk::context::IntoWorkflowClient<'ctx> for #client_ident #client_ty_generics #client_where {
                fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal, key: String) -> Self {
                    Self { ctx, key, #marker_init }
                }
            }
        },
    };

    let handler_fns = svc.handlers.iter().map(|handler| {
        let handler_ident = &handler.ident;
        let handler_literal = Literal::string(&handler.restate_name);

        let argument = match &handler.arg {
            None => quote! {},
            Some(PatType { ty, .. }) => quote! { req: #ty },
        };
        let argument_ty = match &handler.arg {
            None => quote! { () },
            Some(PatType { ty, .. }) => quote! { #ty },
        };
        let res_ty = &handler.output_ok;
        let input = match &handler.arg {
            None => quote! { () },
            Some(_) => quote! { req },
        };
        let request_target = match svc.service_ty {
            ServiceType::Service => quote! {
                ::restate_sdk::context::RequestTarget::service(#service_literal, #handler_literal)
            },
            ServiceType::Object => quote! {
                ::restate_sdk::context::RequestTarget::object(#service_literal, &self.key, #handler_literal)
            },
            ServiceType::Workflow => quote! {
                ::restate_sdk::context::RequestTarget::workflow(#service_literal, &self.key, #handler_literal)
            },
        };

        quote! {
            #vis fn #handler_ident(&self, #argument) -> ::restate_sdk::context::Request<'ctx, #argument_ty, #res_ty> {
                self.ctx.request(#request_target, #input)
            }
        }
    });

    let doc_msg = format!(
        "Client to invoke the `{}` service from another handler.",
        svc.self_ident
    );

    quote! {
        #[doc = #doc_msg]
        #vis struct #client_ident #client_impl_generics #client_where {
            ctx: &'ctx ::restate_sdk::endpoint::ContextInternal,
            #key_field
            #marker_field
        }

        #into_client_impl

        impl #client_impl_generics #client_ident #client_ty_generics #client_where {
            #( #handler_fns )*
        }
    }
}
