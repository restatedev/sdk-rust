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
                    ctx.write_output(res);
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

    // fn struct_client(&self) -> TokenStream2 {
    //     let &Self {
    //         vis,
    //         client_ident,
    //         request_ident,
    //         response_ident,
    //         ..
    //     } = self;
    //
    //     quote! {
    //         #[allow(unused)]
    //         #[derive(Clone, Debug)]
    //         /// The client stub that makes RPC calls to the server. All request methods return
    //         /// [Futures](::core::future::Future).
    //         #vis struct #client_ident<
    //             Stub = ::tarpc::client::Channel<#request_ident, #response_ident>
    //         >(Stub);
    //     }
    // }
    //
    // fn impl_client_new(&self) -> TokenStream2 {
    //     let &Self {
    //         client_ident,
    //         vis,
    //         request_ident,
    //         response_ident,
    //         ..
    //     } = self;
    //
    //     quote! {
    //         impl #client_ident {
    //             /// Returns a new client stub that sends requests over the given transport.
    //             #vis fn new<T>(config: ::tarpc::client::Config, transport: T)
    //                 -> ::tarpc::client::NewClient<
    //                     Self,
    //                     ::tarpc::client::RequestDispatch<#request_ident, #response_ident, T>
    //                 >
    //             where
    //                 T: ::tarpc::Transport<::tarpc::ClientMessage<#request_ident>, ::tarpc::Response<#response_ident>>
    //             {
    //                 let new_client = ::tarpc::client::new(config, transport);
    //                 ::tarpc::client::NewClient {
    //                     client: #client_ident(new_client.client),
    //                     dispatch: new_client.dispatch,
    //                 }
    //             }
    //         }
    //
    //         impl<Stub> ::core::convert::From<Stub> for #client_ident<Stub>
    //             where Stub: ::tarpc::client::stub::Stub<
    //                 Req = #request_ident,
    //                 Resp = #response_ident>
    //         {
    //             /// Returns a new client stub that sends requests over the given transport.
    //             fn from(stub: Stub) -> Self {
    //                 #client_ident(stub)
    //             }
    //
    //         }
    //     }
    // }
    //
    // fn impl_client_rpc_methods(&self) -> TokenStream2 {
    //     let &Self {
    //         client_ident,
    //         request_ident,
    //         response_ident,
    //         method_attrs,
    //         vis,
    //         method_idents,
    //         args,
    //         return_types,
    //         arg_pats,
    //         camel_case_idents,
    //         ..
    //     } = self;
    //
    //     quote! {
    //         impl<Stub> #client_ident<Stub>
    //             where Stub: ::tarpc::client::stub::Stub<
    //                 Req = #request_ident,
    //                 Resp = #response_ident>
    //         {
    //             #(
    //                 #[allow(unused)]
    //                 #( #method_attrs )*
    //                 #vis fn #method_idents(&self, ctx: ::tarpc::context::Context, #( #args ),*)
    //                     -> impl ::core::future::Future<Output = ::core::result::Result<#return_types, ::tarpc::client::RpcError>> + '_ {
    //                     let request = #request_ident::#camel_case_idents { #( #arg_pats ),* };
    //                     let resp = self.0.call(ctx, request);
    //                     async move {
    //                         match resp.await? {
    //                             #response_ident::#camel_case_idents(msg) => ::core::result::Result::Ok(msg),
    //                             _ => ::core::unreachable!(),
    //                         }
    //                     }
    //                 }
    //             )*
    //         }
    //     }
    // }
    //
    // fn emit_warnings(&self) -> TokenStream2 {
    //     self.warnings.iter().map(|w| w.to_token_stream()).collect()
    // }
}

impl<'a> ToTokens for ServiceGenerator<'a> {
    fn to_tokens(&self, output: &mut TokenStream2) {
        output.extend(vec![
            self.trait_service(),
            self.struct_serve(),
            self.impl_service_for_serve(),
            self.impl_discoverable(),
            // self.struct_client(),
            // self.impl_client_new(),
            // self.impl_client_rpc_methods(),
            // self.emit_warnings(),
        ]);
    }
}
