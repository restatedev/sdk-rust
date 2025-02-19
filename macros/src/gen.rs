use crate::ast::{Handler, Object, Service, ServiceInner, ServiceType, Workflow};
use proc_macro2::TokenStream as TokenStream2;
use proc_macro2::{Ident, Literal};
use quote::{format_ident, quote, quote_spanned, ToTokens};
use syn::{Attribute, PatType, Visibility};

pub(crate) struct ServiceGenerator<'a> {
    pub(crate) service_ty: ServiceType,
    pub(crate) restate_name: &'a str,
    pub(crate) service_ident: &'a Ident,
    pub(crate) client_ident: Ident,
    #[cfg(feature = "ingress_client")]
    pub(crate) ingress_ident: Ident,
    #[cfg(feature = "ingress_client")]
    pub(crate) handle_ident: Ident,
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
            #[cfg(feature = "ingress_client")]
            ingress_ident: format_ident!("{}Ingress", s.ident),
            #[cfg(feature = "ingress_client")]
            handle_ident: format_ident!("{}Handle", s.ident),
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

            quote! {
                ::restate_sdk::discovery::Handler {
                    name: ::restate_sdk::discovery::HandlerName::try_from(#handler_literal).expect("Handler name valid"),
                    input: None,
                    output: None,
                    ty: #handler_ty,
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

    #[cfg(feature = "ingress_client")]
    fn struct_ingress(&self) -> TokenStream2 {
        let &Self {
            vis,
            ref ingress_ident,
            ref service_ty,
            service_ident,
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
                    impl<'client> ::restate_sdk::ingress_client::IntoServiceIngress<'client> for #ingress_ident<'client> {
                        fn create_ingress(client: &'client ::restate_sdk::ingress_client::IngressClient) -> Self {
                            Self { client }
                        }
                    }
                }
            }
            ServiceType::Object => quote! {
                impl<'client> ::restate_sdk::ingress_client::IntoObjectIngress<'client> for #ingress_ident<'client> {
                    fn create_ingress(client: &'client ::restate_sdk::ingress_client::IngressClient, key: String) -> Self {
                        Self { client, key }
                    }
                }
            },
            ServiceType::Workflow => quote! {
                impl<'client> ::restate_sdk::ingress_client::IntoWorkflowIngress<'client> for #ingress_ident<'client> {
                    fn create_ingress(client: &'client ::restate_sdk::ingress_client::IngressClient, key: String) -> Self {
                        Self { client, key }
                    }
                }
            },
        };

        let doc_msg = format!(
            "Struct exposing the ingress client to invoke [`{service_ident}`] without a context."
        );
        quote! {
            #[doc = #doc_msg]
            #vis struct #ingress_ident<'client> {
                client: &'client ::restate_sdk::ingress_client::IngressClient,
                #key_field
            }

            #into_client_impl
        }
    }

    #[cfg(feature = "ingress_client")]
    fn impl_ingress(&self) -> TokenStream2 {
        let &Self {
            vis,
            ref ingress_ident,
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
                #vis fn #handler_ident(&self, #argument) -> ::restate_sdk::ingress_client::request::IngressRequest<'client, #argument_ty, #res_ty> {
                    self.client.request(#request_target, #input)
                }
            }
        });

        quote! {
            impl<'client> #ingress_ident<'client> {
                #( #handlers_fns )*
            }
        }
    }

    #[cfg(feature = "ingress_client")]
    fn struct_handle(&self) -> TokenStream2 {
        let &Self {
            vis,
            ref service_ty,
            service_ident,
            restate_name,
            ref handle_ident,
            handlers,
            ..
        } = self;

        let service_literal = Literal::string(restate_name);

        let key_field = match service_ty {
            ServiceType::Service | ServiceType::Workflow => quote! {},
            ServiceType::Object => quote! {
                key: String,
            },
        };

        let into_client_impl = match service_ty {
            ServiceType::Service => quote! {
                impl<'client> ::restate_sdk::ingress_client::IntoServiceHandle<'client> for #handle_ident<'client> {
                    fn create_handle(client: &'client ::restate_sdk::ingress_client::IngressClient) -> Self {
                        Self { client }
                    }
                }
            },
            ServiceType::Object => quote! {
                impl<'client> ::restate_sdk::ingress_client::IntoObjectHandle<'client> for #handle_ident<'client> {
                    fn create_handle(client: &'client ::restate_sdk::ingress_client::IngressClient, key: String) -> Self {
                        Self { client, key }
                    }
                }
            },
            ServiceType::Workflow => quote! {
                impl<'client> ::restate_sdk::ingress_client::IntoWorkflowHandle<'client> for #handle_ident<'client> {
                    fn create_handle(client: &'client ::restate_sdk::ingress_client::IngressClient, id: String) -> Self {
                        Self { result: client.handle(::restate_sdk::ingress_client::handle::HandleTarget::workflow(#service_literal, id)) }
                    }
                }
            },
        };

        let doc_msg =
            format!("Struct exposing the handle to retrieve the result of [`{service_ident}`].");
        match service_ty {
            ServiceType::Service | ServiceType::Object => quote! {
                #[doc = #doc_msg]
                #vis struct #handle_ident<'client> {
                    client: &'client ::restate_sdk::ingress_client::IngressClient,
                    #key_field
                }

                #into_client_impl
            },
            ServiceType::Workflow => {
                let Some(handler) = &handlers
                    .iter()
                    .find(|handler| handler.restate_name == "run")
                else {
                    return quote_spanned! {
                        service_ident.span() => compile_error!("A workflow definition must contain a `run` handler");
                    };
                };
                let res_ty = &handler.output_ok;

                quote! {
                    #[doc = #doc_msg]
                    #vis struct #handle_ident<'client> {
                        result: ::restate_sdk::ingress_client::handle::IngressHandle<'client, #res_ty>,
                    }

                    #into_client_impl
                }
            }
        }
    }

    #[cfg(feature = "ingress_client")]
    fn impl_handle(&self) -> TokenStream2 {
        let &Self {
            vis,
            ref handle_ident,
            handlers,
            restate_name,
            service_ty,
            service_ident,
            ..
        } = self;

        let service_literal = Literal::string(restate_name);

        if let ServiceType::Service | ServiceType::Object = service_ty {
            let handlers_fns = handlers.iter().map(|handler| {
                let handler_ident = &handler.ident;
                let handler_literal = Literal::string(&handler.restate_name);
                let res_ty = &handler.output_ok;

                let handle_target = match service_ty {
                    ServiceType::Service => quote! {
                        ::restate_sdk::ingress_client::handle::HandleTarget::service(#service_literal, #handler_literal, idempotency_key)
                    },
                    ServiceType::Object => quote! {
                        ::restate_sdk::ingress_client::handle::HandleTarget::object(#service_literal, &self.key, #handler_literal, idempotency_key)
                    },
                    ServiceType::Workflow => quote! {
                        ::restate_sdk::ingress_client::handle::HandleTarget::workflow(#service_literal, &self.key, #handler_literal)
                    }
                };
                quote! {
                    #vis fn #handler_ident(&self, idempotency_key: impl Into<String>) -> ::restate_sdk::ingress_client::handle::IngressHandle<'client, #res_ty> {
                        self.client.handle(#handle_target)
                    }
                }
            });

            quote! {
                impl<'client> #handle_ident<'client> {
                    #( #handlers_fns )*
                }
            }
        } else {
            let Some(handler) = &handlers
                .iter()
                .find(|handler| handler.restate_name == "run")
            else {
                return quote_spanned! {
                    service_ident.span() => compile_error!("A workflow definition must contain a `run` handler");
                };
            };
            let res_ty = &handler.output_ok;

            quote! {
                impl<'client> #handle_ident<'client> {
                    #vis async fn attach(self) -> Result<#res_ty, ::restate_sdk::ingress_client::internal::IngressClientError> {
                        self.result.attach().await
                    }

                    #vis async fn output(self) -> Result<#res_ty, ::restate_sdk::ingress_client::internal::IngressClientError> {
                        self.result.output().await
                    }
                }
            }
        }
    }
}

impl<'a> ToTokens for ServiceGenerator<'a> {
    fn to_tokens(&self, output: &mut TokenStream2) {
        output.extend(vec![
            self.trait_service(),
            self.struct_serve(),
            self.impl_service_for_serve(),
            self.impl_discoverable(),
            self.struct_client(),
            self.impl_client(),
            #[cfg(feature = "ingress_client")]
            self.struct_ingress(),
            #[cfg(feature = "ingress_client")]
            self.impl_ingress(),
            #[cfg(feature = "ingress_client")]
            self.struct_handle(),
            #[cfg(feature = "ingress_client")]
            self.impl_handle(),
        ]);
    }
}
