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
use crate::struct_ast::{CommonOptions, OnMaxAttempts, StructHandler, StructService};
use proc_macro2::{Literal, TokenStream as TokenStream2};
use quote::quote;
use syn::{PatType, ext::IdentExt};

/// `Some(<v>)` if set, else `None`, for a primitive that `quote!` can interpolate directly.
fn opt_lit<T: quote::ToTokens>(v: Option<T>) -> TokenStream2 {
    match v {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    }
}

fn opt_doc(doc: &Option<String>) -> TokenStream2 {
    match doc {
        Some(d) => quote! { Some(#d.to_string()) },
        None => quote! { None },
    }
}

fn opt_on_max_attempts(v: Option<OnMaxAttempts>) -> TokenStream2 {
    match v {
        Some(OnMaxAttempts::Pause) => {
            quote! { Some(::restate_sdk::discovery::RetryPolicyOnMaxAttempts::Pause) }
        }
        Some(OnMaxAttempts::Kill) => {
            quote! { Some(::restate_sdk::discovery::RetryPolicyOnMaxAttempts::Kill) }
        }
        None => quote! { None },
    }
}

/// Tokens for the retry-policy fields shared by `discovery::Service` and `discovery::Handler`.
struct RetryPolicyTokens {
    initial_interval: TokenStream2,
    max_interval: TokenStream2,
    max_attempts: TokenStream2,
    exponentiation_factor: TokenStream2,
    on_max_attempts: TokenStream2,
}

fn retry_policy_tokens(opts: &CommonOptions) -> RetryPolicyTokens {
    let r = &opts.retry_policy;
    RetryPolicyTokens {
        initial_interval: opt_lit(r.initial_interval),
        max_interval: opt_lit(r.max_interval),
        max_attempts: opt_lit(r.max_attempts),
        exponentiation_factor: opt_lit(r.factor),
        on_max_attempts: opt_on_max_attempts(r.on_max_attempts),
    }
}

pub(crate) fn generate(svc: &StructService) -> TokenStream2 {
    let stripped_impl = &svc.stripped_impl;
    let dispatcher = dispatcher_block(svc);
    let client = client(svc);
    let ingress_client = ingress_client(svc);

    quote! {
        #stripped_impl
        #dispatcher
        #client
        #ingress_client
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

        let opts = &handler.options;
        let documentation = opt_doc(&handler.documentation);
        let abort_timeout = opt_lit(opts.abort_timeout);
        let inactivity_timeout = opt_lit(opts.inactivity_timeout);
        let journal_retention = opt_lit(opts.journal_retention);
        let idempotency_retention = opt_lit(opts.idempotency_retention);
        // `workflow_completion_retention` is configured at the workflow level but lives on the
        // workflow's (non-shared) `run` handler in the discovery schema.
        let workflow_completion_retention =
            if svc.service_ty == ServiceType::Workflow && !handler.is_shared {
                opt_lit(svc.options.workflow_completion_retention)
            } else {
                quote! { None }
            };
        let enable_lazy_state = opt_lit(opts.enable_lazy_state);
        let ingress_private = opt_lit(opts.ingress_private);
        let retry = retry_policy_tokens(opts);
        let RetryPolicyTokens {
            initial_interval,
            max_interval,
            max_attempts,
            exponentiation_factor,
            on_max_attempts,
        } = retry;

        quote! {
            ::restate_sdk::discovery::Handler {
                name: ::restate_sdk::discovery::HandlerName::try_from(#handler_literal).expect("Handler name valid"),
                input: #input_schema,
                output: #output_schema,
                ty: #handler_ty,
                documentation: #documentation,
                metadata: Default::default(),
                abort_timeout: #abort_timeout,
                inactivity_timeout: #inactivity_timeout,
                journal_retention: #journal_retention,
                idempotency_retention: #idempotency_retention,
                workflow_completion_retention: #workflow_completion_retention,
                enable_lazy_state: #enable_lazy_state,
                ingress_private: #ingress_private,
                retry_policy_initial_interval: #initial_interval,
                retry_policy_max_interval: #max_interval,
                retry_policy_max_attempts: #max_attempts,
                retry_policy_exponentiation_factor: #exponentiation_factor,
                retry_policy_on_max_attempts: #on_max_attempts,
            }
        }
    });

    let opts = &svc.options;
    let documentation = opt_doc(&svc.documentation);
    let abort_timeout = opt_lit(opts.abort_timeout);
    let inactivity_timeout = opt_lit(opts.inactivity_timeout);
    let journal_retention = opt_lit(opts.journal_retention);
    let idempotency_retention = opt_lit(opts.idempotency_retention);
    let enable_lazy_state = opt_lit(opts.enable_lazy_state);
    let ingress_private = opt_lit(opts.ingress_private);
    let RetryPolicyTokens {
        initial_interval,
        max_interval,
        max_attempts,
        exponentiation_factor,
        on_max_attempts,
    } = retry_policy_tokens(opts);

    quote! {
        impl #impl_generics ::restate_sdk::service::Discoverable for #self_ty #where_clause {
            fn discover() -> ::restate_sdk::discovery::Service {
                ::restate_sdk::discovery::Service {
                    ty: #service_ty_token,
                    name: ::restate_sdk::discovery::ServiceName::try_from(#service_literal.to_string())
                        .expect("Service name valid"),
                    handlers: vec![#( #handlers ),*],
                    documentation: #documentation,
                    metadata: Default::default(),
                    abort_timeout: #abort_timeout,
                    inactivity_timeout: #inactivity_timeout,
                    journal_retention: #journal_retention,
                    idempotency_retention: #idempotency_retention,
                    enable_lazy_state: #enable_lazy_state,
                    ingress_private: #ingress_private,
                    retry_policy_initial_interval: #initial_interval,
                    retry_policy_max_interval: #max_interval,
                    retry_policy_max_attempts: #max_attempts,
                    retry_policy_exponentiation_factor: #exponentiation_factor,
                    retry_policy_on_max_attempts: #on_max_attempts,
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

    let (marker_field, marker_init) = client_marker(&svc.generics);

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
        let HandlerClientParts {
            handler_ident,
            handler_literal,
            argument,
            argument_ty,
            res_ty,
            input,
        } = handler_client_parts(handler);
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

/// The pieces of a handler signature and invocation that are shared by the durable and ingress
/// generated clients. Handler parameter patterns intentionally do not leak into either client API:
/// callers always pass a naturally named `req` value.
struct HandlerClientParts<'a> {
    handler_ident: &'a syn::Ident,
    handler_literal: Literal,
    argument: TokenStream2,
    argument_ty: TokenStream2,
    res_ty: &'a syn::Type,
    input: TokenStream2,
}

fn handler_client_parts(handler: &StructHandler) -> HandlerClientParts<'_> {
    let (argument, argument_ty, input) = match &handler.arg {
        None => (quote! {}, quote! { () }, quote! { () }),
        Some(PatType { ty, .. }) => (quote! { req: #ty }, quote! { #ty }, quote! { req }),
    };

    HandlerClientParts {
        handler_ident: &handler.ident,
        handler_literal: Literal::string(&handler.restate_name),
        argument,
        argument_ty,
        res_ty: &handler.output_ok,
        input,
    }
}

/// A `PhantomData` marker so service generic parameters remain used even when no handler wire type
/// mentions them. The executor parameter of an ingress client is used by its `Client` field and is
/// therefore deliberately not included here.
fn client_marker(generics: &syn::Generics) -> (TokenStream2, TokenStream2) {
    let marker_types: Vec<TokenStream2> = generics
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

    if marker_types.is_empty() {
        (quote! {}, quote! {})
    } else {
        (
            quote! { __restate_marker: ::core::marker::PhantomData<(#(#marker_types,)*)>, },
            quote! { __restate_marker: ::core::marker::PhantomData, },
        )
    }
}

/// Pick an executor type parameter that cannot collide with a generic declared by the user's
/// service impl. The deliberately uncommon base still gets checked because proc-macro output must
/// remain correct for adversarial (and generated) Rust source.
fn ingress_executor_ident(generics: &syn::Generics) -> syn::Ident {
    let mut candidate = "__RestateIngressExecutor".to_owned();
    while generics.params.iter().any(|param| match param {
        syn::GenericParam::Type(ty) => ty.ident.unraw() == candidate,
        syn::GenericParam::Const(konst) => konst.ident.unraw() == candidate,
        syn::GenericParam::Lifetime(_) => false,
    }) {
        candidate.push('_');
    }
    quote::format_ident!("{candidate}")
}

/// The `XIngressClient` generated alongside the durable `XClient` for the struct-based macro API.
/// It is transport-neutral: no dependency feature is referenced in downstream macro output.
fn ingress_client(svc: &StructService) -> TokenStream2 {
    let vis = &svc.vis;
    let client_ident = quote::format_ident!("{}IngressClient", svc.self_ident);
    let service_literal = Literal::string(&svc.restate_name);
    let executor_ident = ingress_executor_ident(&svc.generics);

    // Ingress client generics = the service's generics followed by a fresh executor parameter.
    let mut client_generics = svc.generics.clone();
    client_generics.params.push(syn::parse_quote! {
        #executor_ident: ::restate_sdk::ingress::RequestExecutor
    });
    let (client_impl_generics, client_ty_generics, client_where) = client_generics.split_for_impl();

    let (marker_field, marker_init) = client_marker(&svc.generics);
    let (key_field, constructor_argument, key_init) = match svc.service_ty {
        ServiceType::Service => (quote! {}, quote! {}, quote! {}),
        ServiceType::Object | ServiceType::Workflow => (
            quote! { key: ::std::string::String, },
            quote! { key: impl ::core::convert::Into<::std::string::String>, },
            quote! { key: key.into(), },
        ),
    };

    let handler_fns = svc.handlers.iter().map(|handler| {
        let HandlerClientParts {
            handler_ident,
            handler_literal,
            argument,
            argument_ty,
            res_ty,
            input,
        } = handler_client_parts(handler);
        let request_target = match svc.service_ty {
            ServiceType::Service => quote! {
                ::restate_sdk::ingress::RequestTarget::service(
                    #service_literal,
                    #handler_literal,
                )
            },
            ServiceType::Object => quote! {
                ::restate_sdk::ingress::RequestTarget::object(
                    #service_literal,
                    self.key.clone(),
                    #handler_literal,
                )
            },
            ServiceType::Workflow => quote! {
                ::restate_sdk::ingress::RequestTarget::workflow(
                    #service_literal,
                    self.key.clone(),
                    #handler_literal,
                )
            },
        };

        quote! {
            #vis fn #handler_ident(
                &self,
                #argument
            ) -> ::restate_sdk::ingress::Request<#executor_ident, #argument_ty, #res_ty> {
                self.client.request(#request_target, #input)
            }
        }
    });

    let doc_msg = format!(
        "Client to invoke the `{}` service through Restate ingress.",
        svc.self_ident
    );

    quote! {
        #[doc = #doc_msg]
        #vis struct #client_ident #client_impl_generics #client_where {
            client: ::restate_sdk::ingress::Client<#executor_ident>,
            #key_field
            #marker_field
        }

        impl #client_impl_generics #client_ident #client_ty_generics #client_where {
            #vis fn from_client(
                client: ::restate_sdk::ingress::Client<#executor_ident>,
                #constructor_argument
            ) -> Self {
                Self {
                    client,
                    #key_init
                    #marker_init
                }
            }

            #( #handler_fns )*
        }
    }
}
