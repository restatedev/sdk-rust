use crate::ast::{Handler, Service};
use proc_macro2::TokenStream as TokenStream2;
use proc_macro2::{Ident, Literal};
use quote::{format_ident, quote, ToTokens};
use syn::{parse_quote, Attribute, ReturnType, Type, Visibility};

#[derive(Clone, Copy, Debug)]
pub(crate) enum ServiceType {
    Service,
    VirtualObject,
    Workflow,
}

pub(crate) struct ServiceGenerator<'a> {
    pub(crate) service_ty: ServiceType,
    pub(crate) service_ident: &'a Ident,
    pub(crate) serve_ident: Ident,
    pub(crate) vis: &'a Visibility,
    pub(crate) attrs: &'a [Attribute],
    pub(crate) handlers: &'a [Handler],
}

impl<'a> ServiceGenerator<'a> {
    pub(crate) fn new(service_ty: ServiceType, s: &'a Service) -> Self {
        ServiceGenerator {
            service_ty,
            service_ident: &s.ident,
            serve_ident: format_ident!("Serve{}", s.ident),
            vis: &s.vis,
            attrs: &s.attrs,
            handlers: &s.handlers,
        }
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

        let unit_type: &Type = &parse_quote!(());

        let handler_fns = handlers
            .iter()
            .map(
                |Handler { attrs, ident, arg, is_shared, output }| {
                    let args = arg.iter();

                    let ctx = match (&service_ty, is_shared) {
                        (ServiceType::Service, _) => quote! { ::restate_sdk::prelude::Context },
                        (ServiceType::VirtualObject, true) => quote! { ::restate_sdk::prelude::SharedObjectContext },
                        (ServiceType::VirtualObject, false) => quote! { ::restate_sdk::prelude::ObjectContext },
                        (ServiceType::Workflow, true) => quote! { ::restate_sdk::prelude::SharedWorkflowContext },
                        (ServiceType::Workflow, false) => quote! { ::restate_sdk::prelude::WorkflowContext },
                    };

                    let output = match output {
                        ReturnType::Type(_, ref ty) => ty.as_ref(),
                        ReturnType::Default => unit_type,
                    };
                    quote! {
                        #( #attrs )*
                        fn #ident(&self, context: #ctx, #( #args ),*) -> impl std::future::Future<Output=#output> + ::core::marker::Send;
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

            let handler_literal = Literal::string(&handler_ident.to_string());

            quote! {
                #handler_literal => {
                    #get_input_and_call
                    let res = fut.await;
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
            ..
        } = self;

        let service_literal = Literal::string(&service_ident.to_string());

        let service_ty = match service_ty {
            ServiceType::Service => quote! { ::restate_sdk::discovery::ServiceType::Service },
            ServiceType::VirtualObject => {
                quote! { ::restate_sdk::discovery::ServiceType::VirtualObject }
            }
            ServiceType::Workflow => quote! { ::restate_sdk::discovery::ServiceType::Workflow },
        };

        let handlers = handlers.iter().map(|handler| {
            let handler_literal = Literal::string(&handler.ident.to_string());

            let handler_ty = if handler.is_shared {
                quote! { Some(::restate_sdk::discovery::HandlerType::Shared) }
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
                        ty: #service_ty,
                        name: ::restate_sdk::discovery::ServiceName::try_from(#service_literal.to_string())
                            .expect("Service name valid"),
                        handlers: vec![#( #handlers ),*],
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
        ]);
    }
}
