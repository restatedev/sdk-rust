use crate::ast::{Handler, Object, Service, ServiceInner, ServiceType, Workflow};
use proc_macro2::TokenStream as TokenStream2;
use proc_macro2::{Ident, Literal};
use quote::{ToTokens, format_ident, quote};
use syn::{Attribute, PatType, Visibility};

pub(crate) struct ServiceGenerator<'a> {
    pub(crate) service_ty: ServiceType,
    pub(crate) restate_name: &'a str,
    pub(crate) service_ident: &'a Ident,
    pub(crate) client_ident: Ident,
    pub(crate) serve_ident: Ident,
    pub(crate) vis: &'a Visibility,
    pub(crate) attrs: &'a [Attribute],
    pub(crate) handlers: &'a [Handler],
}

impl<'a> ServiceGenerator<'a> {
    fn new(service_ty: ServiceType, s: &'a ServiceInner) -> Self {
        ServiceGenerator {
            service_ty,
            restate_name: &s.restate_name,
            service_ident: &s.ident,
            client_ident: format_ident!("{}Client", s.ident),
            serve_ident: format_ident!("Serve{}", s.ident),
            vis: &s.vis,
            attrs: &s.attrs,
            handlers: &s.handlers,
        }
    }

    pub(crate) fn new_service(s: &'a Service) -> Self {
        Self::new(ServiceType::Service, &s.0)
    }

    pub(crate) fn new_object(s: &'a Object) -> Self {
        Self::new(ServiceType::Object, &s.0)
    }

    pub(crate) fn new_workflow(s: &'a Workflow) -> Self {
        Self::new(ServiceType::Workflow, &s.0)
    }

    fn trait_service(&self) -> TokenStream2 {
        let Self {
            attrs,
            handlers,
            vis,
            service_ident,
            service_ty,
            serve_ident,
            ..
        } = self;

        let handler_fns = handlers
            .iter()
            .map(
                |Handler { attrs, ident, arg, is_shared, output_ok, output_err, .. }| {
                    let args = arg.iter();

                    let ctx = match (&service_ty, is_shared) {
                        (ServiceType::Service, _) => quote! { ::restate_sdk::prelude::Context },
                        (ServiceType::Object, true) => quote! { ::restate_sdk::prelude::SharedObjectContext },
                        (ServiceType::Object, false) => quote! { ::restate_sdk::prelude::ObjectContext },
                        (ServiceType::Workflow, true) => quote! { ::restate_sdk::prelude::SharedWorkflowContext },
                        (ServiceType::Workflow, false) => quote! { ::restate_sdk::prelude::WorkflowContext },
                    };

                    quote! {
                        #( #attrs )*
                        fn #ident(&self, context: #ctx, #( #args ),*) -> impl std::future::Future<Output=Result<#output_ok, #output_err>> + ::core::marker::Send;
                    }
                },
            );

        quote! {
            #( #attrs )*
            #vis trait #service_ident: ::core::marker::Sized {
                #( #handler_fns )*

                /// Returns a serving function to use with [::restate_sdk::endpoint::Builder::with_service].
                fn serve(self) -> #serve_ident<Self> {
                    #serve_ident { service: ::std::sync::Arc::new(self) }
                }
            }
        }
    }

    fn struct_serve(&self) -> TokenStream2 {
        let &Self {
            vis,
            ref serve_ident,
            ..
        } = self;

        quote! {
            /// Struct implementing [::restate_sdk::service::Service], to be used with [::restate_sdk::endpoint::Builder::with_service].
            #[derive(Clone)]
            #vis struct #serve_ident<S> {
                service: ::std::sync::Arc<S>,
            }
        }
    }

    fn impl_service_for_serve(&self) -> TokenStream2 {
        let Self {
            serve_ident,
            service_ident,
            handlers,
            ..
        } = self;

        let match_arms = handlers.iter().map(|handler| {
            let handler_ident = &handler.ident;

            let get_input_and_call = if handler.arg.is_some() {
                quote! {
                    let (input, metadata) = ctx.input().await;
                    let fut = S::#handler_ident(&service_clone, (&ctx, metadata).into(), input);
                }
            } else {
                quote! {
                    let (_, metadata) = ctx.input::<()>().await;
                    let fut = S::#handler_ident(&service_clone, (&ctx, metadata).into());
                }
            };

            let handler_literal = Literal::string(&handler.restate_name);

            quote! {
                #handler_literal => {
                    #get_input_and_call
                    let res = fut.await.map_err(::restate_sdk::errors::HandlerError::from);
                    ctx.handle_handler_result(res);
                    ctx.end();
                    Ok(())
                }
            }
        });

        quote! {
            impl<S> ::restate_sdk::service::Service for #serve_ident<S>
                where S: #service_ident + Send + Sync + 'static,
            {
                type Future = ::restate_sdk::service::ServiceBoxFuture;

                fn handle(&self, ctx: ::restate_sdk::endpoint::ContextInternal) -> Self::Future {
                    let service_clone = ::std::sync::Arc::clone(&self.service);
                    Box::pin(async move {
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
        }
    }

    fn impl_discoverable(&self) -> TokenStream2 {
        let Self {
            service_ty,
            serve_ident,
            service_ident,
            handlers,
            restate_name,
            ..
        } = self;

        let service_literal = Literal::string(restate_name);

        let service_ty_token = match service_ty {
            ServiceType::Service => quote! { ::restate_sdk::discovery::ServiceType::Service },
            ServiceType::Object => {
                quote! { ::restate_sdk::discovery::ServiceType::VirtualObject }
            }
            ServiceType::Workflow => quote! { ::restate_sdk::discovery::ServiceType::Workflow },
        };

        let handlers = handlers.iter().map(|handler| {
            let handler_literal = Literal::string(&handler.restate_name);

            let handler_ty = if handler.is_shared {
                quote! { Some(::restate_sdk::discovery::HandlerType::Shared) }
            } else if *service_ty == ServiceType::Workflow {
                quote! { Some(::restate_sdk::discovery::HandlerType::Workflow) }
            } else {
                // Macro has same defaulting rules of the discovery manifest
                quote! { None }
            };

            let lazy_state = if handler.is_lazy_state {
                quote! { Some(true) }
            } else {
                quote! { None}
            };

            let input_schema = match &handler.arg {
                Some(PatType { ty, .. }) => {
                    quote! {
                        Some(::restate_sdk::discovery::InputPayload::from_metadata::<#ty>())
                    }
                }
                None => quote! {
                    Some(::restate_sdk::discovery::InputPayload::empty())
                }
            };

            let output_ty = &handler.output_ok;
            let output_schema = match output_ty {
                syn::Type::Tuple(tuple) if tuple.elems.is_empty() => quote! {
                    Some(::restate_sdk::discovery::OutputPayload::empty())
                },
                _ => quote! {
                    Some(::restate_sdk::discovery::OutputPayload::from_metadata::<#output_ty>())
                }
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
                    enable_lazy_state: #lazy_state,
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
            impl<S> ::restate_sdk::service::Discoverable for #serve_ident<S>
                where S: #service_ident,
            {
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

    fn struct_client(&self) -> TokenStream2 {
        let &Self {
            vis,
            ref client_ident,
            // service_ident,
            ref service_ty,
            ..
        } = self;

        let key_field = match service_ty {
            ServiceType::Service => quote! {},
            ServiceType::Object | ServiceType::Workflow => quote! {
                key: String,
            },
        };

        let into_client_impl = match service_ty {
            ServiceType::Service => {
                quote! {
                    impl<'ctx> ::restate_sdk::context::IntoServiceClient<'ctx> for #client_ident<'ctx> {
                        fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal) -> Self {
                            Self { ctx }
                        }
                    }
                }
            }
            ServiceType::Object => quote! {
                impl<'ctx> ::restate_sdk::context::IntoObjectClient<'ctx> for #client_ident<'ctx> {
                    fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal, key: String) -> Self {
                        Self { ctx, key }
                    }
                }
            },
            ServiceType::Workflow => quote! {
                impl<'ctx> ::restate_sdk::context::IntoWorkflowClient<'ctx> for #client_ident<'ctx> {
                    fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal, key: String) -> Self {
                        Self { ctx, key }
                    }
                }
            },
        };

        quote! {
            /// Struct exposing the client to invoke [#service_ident] from another service.
            #vis struct #client_ident<'ctx> {
                ctx: &'ctx ::restate_sdk::endpoint::ContextInternal,
                #key_field
            }

            #into_client_impl
        }
    }

    fn impl_client(&self) -> TokenStream2 {
        let &Self {
            vis,
            ref client_ident,
            service_ident,
            handlers,
            restate_name,
            service_ty,
            ..
        } = self;

        let service_literal = Literal::string(restate_name);

        let handlers_fns = handlers.iter().map(|handler| {
            let handler_ident = &handler.ident;
            let handler_literal = Literal::string(&handler.restate_name);

            let argument = match &handler.arg {
                None => quote! {},
                Some(PatType {
                         ty, ..
                     }) => quote! { req: #ty }
            };
            let argument_ty = match &handler.arg {
                None => quote! { () },
                Some(PatType {
                         ty, ..
                     }) => quote! { #ty }
            };
            let res_ty = &handler.output_ok;
            let input =  match &handler.arg {
                None => quote! { () },
                Some(_) => quote! { req }
            };
            let request_target = match service_ty {
                ServiceType::Service => quote! {
                    ::restate_sdk::context::RequestTarget::service(#service_literal, #handler_literal)
                },
                ServiceType::Object => quote! {
                    ::restate_sdk::context::RequestTarget::object(#service_literal, &self.key, #handler_literal)
                },
                ServiceType::Workflow => quote! {
                    ::restate_sdk::context::RequestTarget::workflow(#service_literal, &self.key, #handler_literal)
                }
            };

            quote! {
                #vis fn #handler_ident(&self, #argument) -> ::restate_sdk::context::Request<'ctx, #argument_ty, #res_ty> {
                    self.ctx.request(#request_target, #input)
                }
            }
        });

        let doc_msg = format!(
            "Struct exposing the client to invoke [`{service_ident}`] from another service."
        );
        quote! {
            #[doc = #doc_msg]
            impl<'ctx> #client_ident<'ctx> {
                #( #handlers_fns )*
            }
        }
    }
}

impl ToTokens for ServiceGenerator<'_> {
    fn to_tokens(&self, output: &mut TokenStream2) {
        output.extend(vec![
            self.trait_service(),
            self.struct_serve(),
            self.impl_service_for_serve(),
            self.impl_discoverable(),
            self.struct_client(),
            self.impl_client(),
        ]);
    }
}
