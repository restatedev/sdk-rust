use crate::ast::{Handler, Object, Service, ServiceInner, ServiceType, Workflow};
use proc_macro2::TokenStream as TokenStream2;
use proc_macro2::{Ident, Literal};
use quote::{ToTokens, format_ident, quote};

pub(crate) struct ServiceGenerator<'a> {
    service: ServiceScope<'a>,
    handlers: Vec<HandlerScope<'a>>,
}

impl<'a> ServiceGenerator<'a> {
    pub(crate) fn new_service(s: &'a Service) -> Self {
        Self::new(ServiceType::Service, &s.0)
    }

    pub(crate) fn new_object(s: &'a Object) -> Self {
        Self::new(ServiceType::Object, &s.0)
    }

    pub(crate) fn new_workflow(s: &'a Workflow) -> Self {
        Self::new(ServiceType::Workflow, &s.0)
    }

    fn new(service_ty: ServiceType, s: &'a ServiceInner) -> Self {
        ServiceGenerator {
            service: ServiceScope::new(service_ty, s),
            handlers: s.handlers.iter().map(HandlerScope::from_handler).collect(),
        }
    }

    fn trait_service(&self) -> TokenStream2 {
        self.service.trait_service_tokens(self.handlers.iter())
    }

    fn struct_serve(&self) -> TokenStream2 {
        self.service.struct_serve_tokens()
    }

    fn impl_service_for_serve(&self) -> TokenStream2 {
        self.service
            .impl_service_for_serve_tokens(self.handlers.iter())
    }

    fn impl_discoverable(&self) -> TokenStream2 {
        self.service.impl_discoverable_tokens(self.handlers.iter())
    }

    fn struct_client(&self) -> TokenStream2 {
        self.service.struct_client_tokens()
    }

    fn impl_client(&self) -> TokenStream2 {
        self.service.impl_client_tokens(self.handlers.iter())
    }

    fn struct_ingress(&self) -> TokenStream2 {
        self.service.struct_ingress_tokens()
    }

    fn impl_ingress(&self) -> TokenStream2 {
        self.service.impl_ingress_tokens(self.handlers.iter())
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
            self.struct_ingress(),
            self.impl_ingress(),
        ]);
    }
}

struct ServiceScope<'a> {
    service_ty: ServiceType,
    service: &'a ServiceInner,
    service_literal: Literal,
    client_ident: Ident,
    ingress_ident: Ident,
    serve_ident: Ident,
}

impl ServiceScope<'_> {
    fn new<'a>(service_ty: ServiceType, s: &'a ServiceInner) -> ServiceScope<'a> {
        let service_literal = Literal::string(&s.restate_name);
        let client_ident = format_ident!("{}Client", s.ident);
        let ingress_ident = format_ident!("Ingress{}", s.ident);
        let serve_ident = format_ident!("Serve{}", s.ident);

        ServiceScope {
            service_ty,
            service: s,
            service_literal,
            client_ident,
            ingress_ident,
            serve_ident,
        }
    }

    fn trait_service_tokens<'a>(
        &self,
        handlers: impl IntoIterator<Item = &'a HandlerScope<'a>>,
    ) -> TokenStream2 {
        let service_ident = &self.service.ident;
        let serve_ident = &self.serve_ident;
        let vis = &self.service.vis;
        let attrs = &self.service.attrs;
        let handler_fns = handlers
            .into_iter()
            .map(|handler| handler.trait_method_tokens(self));

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

    fn struct_serve_tokens(&self) -> TokenStream2 {
        let vis = &self.service.vis;
        let serve_ident = &self.serve_ident;

        quote! {
            /// Struct implementing [::restate_sdk::service::Service], to be used with [::restate_sdk::endpoint::Builder::with_service].
            #[derive(Clone)]
            #vis struct #serve_ident<S> {
                service: ::std::sync::Arc<S>,
            }
        }
    }

    fn impl_service_for_serve_tokens<'a>(
        &self,
        handlers: impl IntoIterator<Item = &'a HandlerScope<'a>>,
    ) -> TokenStream2 {
        let serve_ident = &self.serve_ident;
        let service_ident = &self.service.ident;
        let match_arms = handlers
            .into_iter()
            .map(HandlerScope::serve_match_arm_tokens);

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

    fn impl_discoverable_tokens<'a>(
        &self,
        handlers: impl IntoIterator<Item = &'a HandlerScope<'a>>,
    ) -> TokenStream2 {
        let serve_ident = &self.serve_ident;
        let service_ident = &self.service.ident;
        let service_literal = &self.service_literal;
        let service_ty_token = self.discovery_service_type_tokens();
        let handlers = handlers
            .into_iter()
            .map(|handler| handler.discovery_handler_tokens(self));

        quote! {
            impl<S> ::restate_sdk::service::Discoverable for #serve_ident<S>
                where S: #service_ident,
            {
                fn discover() -> ::restate_sdk::discovery::Service {
                    use  ::restate_sdk::discovery;
                    discovery::Service {
                        name: discovery::ServiceName::try_from(#service_literal).unwrap(),
                        ty: #service_ty_token,
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

    fn discovery_service_type_tokens(&self) -> TokenStream2 {
        match self.service_ty {
            ServiceType::Service => quote! { ::restate_sdk::discovery::ServiceType::Service },
            ServiceType::Object => quote! { ::restate_sdk::discovery::ServiceType::VirtualObject },
            ServiceType::Workflow => quote! { ::restate_sdk::discovery::ServiceType::Workflow },
        }
    }

    fn into_client_impl_tokens(&self) -> TokenStream2 {
        let client_ident = &self.client_ident;
        match self.service_ty {
            ServiceType::Service => quote! {
                impl<'ctx> ::restate_sdk::context::IntoServiceClient<'ctx> for #client_ident<'ctx> {
                    fn create_client(ctx: &'ctx ::restate_sdk::endpoint::ContextInternal) -> Self {
                        Self { ctx }
                    }
                }
            },
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
        }
    }

    fn into_impl_ingress_tokens(&self) -> TokenStream2 {
        let client_ident = &self.client_ident;
        let ingress_ident = &self.ingress_ident;
        match self.service_ty {
            ServiceType::Service => quote! {
                impl<'ctx> ::restate_sdk::ingress::builder::IntoServiceRequest for #client_ident<'ctx> {
                    type Request<'a> = #ingress_ident<'a>;

                    fn create_request<'a>(client: &'a ::restate_sdk::ingress::Client) -> Self::Request<'a> {
                        #ingress_ident {
                            client,
                        }
                    }
                }
            },
            ServiceType::Object => quote! {
                impl<'ctx> ::restate_sdk::ingress::builder::IntoObjectRequest for #client_ident<'ctx> {
                    type Request<'a> = #ingress_ident<'a>;

                    fn create_request<'a>(client: &'a ::restate_sdk::ingress::Client, key: String) -> Self::Request<'a> {
                        #ingress_ident {
                            client,
                            key,
                        }
                    }
                }
            },
            ServiceType::Workflow => quote! {
                impl<'ctx> ::restate_sdk::ingress::builder::IntoWorkflowRequest for #client_ident<'ctx> {
                    type Request<'a> = #ingress_ident<'a>;

                    fn create_request<'a>(client: &'a ::restate_sdk::ingress::Client, key: String) -> Self::Request<'a> {
                        #ingress_ident {
                            client,
                            key,
                        }
                    }
                }
            },
        }
    }

    fn struct_ingress_tokens(&self) -> TokenStream2 {
        let vis = &self.service.vis;
        let ingress_ident = &self.ingress_ident;
        let key_field = self.key_field_tokens();
        let into_ingress_client_impl = self.into_impl_ingress_tokens();
        let service_ident = &self.service.ident;
        let doc_msg = format!(
            "Struct exposing the ingress client to invoke [`{service_ident}`] from from the ingress API."
        );

        quote! {
            #[doc = #doc_msg]
            #vis struct #ingress_ident<'a> {
                client: &'a ::restate_sdk::ingress::Client,
                #key_field
            }

            #into_ingress_client_impl
        }
    }

    fn struct_client_tokens(&self) -> TokenStream2 {
        let vis = &self.service.vis;
        let client_ident = &self.client_ident;
        let key_field = self.key_field_tokens();
        let into_client_impl_tokens = self.into_client_impl_tokens();
        let service_ident = &self.service.ident;
        let doc_msg = format!(
            "Struct exposing the client to invoke [`{service_ident}`] from another service."
        );

        quote! {
            #[doc = #doc_msg]
            #vis struct #client_ident<'ctx> {
                ctx: &'ctx ::restate_sdk::endpoint::ContextInternal,
                #key_field
            }

            #into_client_impl_tokens
        }
    }

    fn key_field_tokens(&self) -> TokenStream2 {
        match self.service_ty {
            ServiceType::Service => quote! {},
            ServiceType::Object | ServiceType::Workflow => quote! { key: String, },
        }
    }

    fn impl_client_tokens<'a>(
        &self,
        handlers: impl IntoIterator<Item = &'a HandlerScope<'a>>,
    ) -> TokenStream2 {
        let client_ident = &self.client_ident;
        let handlers_fns = handlers
            .into_iter()
            .map(|handler| handler.client_method_tokens(self));
        let service_ident = &self.service.ident;
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

    fn impl_ingress_tokens<'a>(
        &self,
        handlers: impl IntoIterator<Item = &'a HandlerScope<'a>>,
    ) -> TokenStream2 {
        let ingress_ident = &self.ingress_ident;
        let handler_fns = handlers
            .into_iter()
            .map(|handler| handler.ingress_method_tokens(self));

        quote! {
            impl #ingress_ident<'_> {
                #( #handler_fns )*
            }
        }
    }
}

struct HandlerScope<'a> {
    handler: &'a Handler,
    arg_ty: Option<&'a syn::Type>,
    handler_literal: Literal,
}

impl HandlerScope<'_> {
    fn from_handler(handler: &Handler) -> HandlerScope<'_> {
        let arg_ty = handler.arg.as_ref().map(|pat_ty| pat_ty.ty.as_ref());
        let handler_literal = Literal::string(&handler.restate_name);

        HandlerScope {
            handler,
            arg_ty,
            handler_literal,
        }
    }

    fn trait_method_tokens(&self, service: &ServiceScope<'_>) -> TokenStream2 {
        let attrs = &self.handler.attrs;
        let ident = &self.handler.ident;
        let ctx = self.handler_client_tokens(service);
        let args = self.handler.arg.iter();
        let output_ok = &self.handler.output_ok;
        let output_err = &self.handler.output_err;

        quote! {
            #( #attrs )*
            fn #ident(&self, context: #ctx, #( #args ),*) -> impl std::future::Future<Output=Result<#output_ok, #output_err>> + ::core::marker::Send;
        }
    }

    fn handler_client_tokens(&self, service: &ServiceScope<'_>) -> TokenStream2 {
        match (service.service_ty, self.handler.is_shared) {
            (ServiceType::Service, _) => quote! { ::restate_sdk::prelude::Context },
            (ServiceType::Object, true) => quote! { ::restate_sdk::prelude::SharedObjectContext },
            (ServiceType::Object, false) => quote! { ::restate_sdk::prelude::ObjectContext },
            (ServiceType::Workflow, true) => {
                quote! { ::restate_sdk::prelude::SharedWorkflowContext }
            }
            (ServiceType::Workflow, false) => quote! { ::restate_sdk::prelude::WorkflowContext },
        }
    }

    fn serve_match_arm_tokens(&self) -> TokenStream2 {
        let handler_ident = &self.handler.ident;
        let get_input_and_call = if self.arg_ty.is_some() {
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
        let handler_literal = &self.handler_literal;

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

    fn discovery_handler_tokens(&self, service: &ServiceScope<'_>) -> TokenStream2 {
        let handler_literal = &self.handler_literal;
        let handler_ty = self.discovery_handler_type_tokens(service);
        let input_schema = self.discovery_input_schema_tokens();
        let output_schema = self.discovery_output_schema_tokens();
        let lazy_state = self.discovery_lazy_state_tokens();

        quote! {
            ::restate_sdk::discovery::Handler {
                name: ::restate_sdk::discovery::HandlerName::try_from(#handler_literal).expect("Handler name valid"),
                ty: #handler_ty,
                input: #input_schema,
                output: #output_schema,
                enable_lazy_state: #lazy_state,
                documentation: None,
                metadata: Default::default(),
                abort_timeout: None,
                inactivity_timeout: None,
                journal_retention: None,
                idempotency_retention: None,
                workflow_completion_retention: None,
                ingress_private: None,
                retry_policy_initial_interval: None,
                retry_policy_max_interval: None,
                retry_policy_max_attempts: None,
                retry_policy_exponentiation_factor: None,
                retry_policy_on_max_attempts: None,
            }
        }
    }

    fn discovery_handler_type_tokens(&self, service: &ServiceScope<'_>) -> TokenStream2 {
        if self.handler.is_shared {
            quote! { Some(::restate_sdk::discovery::HandlerType::Shared) }
        } else if service.service_ty == ServiceType::Workflow {
            quote! { Some(::restate_sdk::discovery::HandlerType::Workflow) }
        } else {
            // Macro has same defaulting rules of the discovery manifest
            quote! { None }
        }
    }

    fn discovery_input_schema_tokens(&self) -> TokenStream2 {
        match self.arg_ty {
            Some(ty) => quote! {
                Some(::restate_sdk::discovery::InputPayload::from_metadata::<#ty>())
            },
            None => quote! {
                Some(::restate_sdk::discovery::InputPayload::empty())
            },
        }
    }

    fn discovery_output_schema_tokens(&self) -> TokenStream2 {
        let output_ty = &self.handler.output_ok;
        match output_ty {
            syn::Type::Tuple(tuple) if tuple.elems.is_empty() => quote! {
                Some(::restate_sdk::discovery::OutputPayload::empty())
            },
            _ => quote! {
                Some(::restate_sdk::discovery::OutputPayload::from_metadata::<#output_ty>())
            },
        }
    }

    fn discovery_lazy_state_tokens(&self) -> TokenStream2 {
        if self.handler.is_lazy_state {
            quote! { Some(true) }
        } else {
            quote! { None }
        }
    }

    fn client_method_tokens(&self, service: &ServiceScope<'_>) -> TokenStream2 {
        let vis = &service.service.vis;
        let handler_ident = &self.handler.ident;
        let argument = self.argument_tokens();
        let argument_ty = self.argument_type_tokens();
        let response_ty = &self.handler.output_ok;
        let request_target = self.context_request_target_tokens(service);
        let input = self.input_tokens();

        quote! {
            #vis fn #handler_ident(&self, #argument) -> ::restate_sdk::context::Request<'ctx, #argument_ty, #response_ty> {
                self.ctx.request(#request_target, #input)
            }
        }
    }

    fn ingress_method_tokens(&self, service: &ServiceScope<'_>) -> TokenStream2 {
        let vis = &service.service.vis;
        let handler_ident = &self.handler.ident;
        let argument = self.argument_tokens();
        let response_ty = &self.handler.output_ok;
        let request_call = self.ingress_request_call_tokens(service);

        quote! {
            #vis fn #handler_ident(&self, #argument) -> ::restate_sdk::ingress::Request<#response_ty> {
                #request_call
            }
        }
    }

    fn context_request_target_tokens(&self, service: &ServiceScope<'_>) -> TokenStream2 {
        let service_literal = &service.service_literal;
        let handler_literal = &self.handler_literal;
        match service.service_ty {
            ServiceType::Service => quote! {
                ::restate_sdk::context::RequestTarget::service(#service_literal, #handler_literal)
            },
            ServiceType::Object => quote! {
                ::restate_sdk::context::RequestTarget::object(#service_literal, &self.key, #handler_literal)
            },
            ServiceType::Workflow => quote! {
                ::restate_sdk::context::RequestTarget::workflow(#service_literal, &self.key, #handler_literal)
            },
        }
    }

    fn ingress_request_call_tokens(&self, service: &ServiceScope<'_>) -> TokenStream2 {
        let service_literal = &service.service_literal;
        let handler_literal = &self.handler_literal;
        let input = self.input_tokens();
        match service.service_ty {
            ServiceType::Service => quote! {
                ::restate_sdk::ingress::builder::service(self.client, concat!("/", #service_literal, "/", #handler_literal), #input)
            },
            ServiceType::Object => quote! {
                ::restate_sdk::ingress::builder::object(self.client, #service_literal, &self.key, #handler_literal, #input)
            },
            ServiceType::Workflow => quote! {
                ::restate_sdk::ingress::builder::workflow(self.client, #service_literal, &self.key, #handler_literal, #input)
            },
        }
    }

    fn argument_tokens(&self) -> TokenStream2 {
        match self.arg_ty {
            None => quote! {},
            Some(arg_ty) => quote! { req: #arg_ty },
        }
    }

    fn argument_type_tokens(&self) -> TokenStream2 {
        match self.arg_ty {
            None => quote! { () },
            Some(arg_ty) => quote! { #arg_ty },
        }
    }

    fn input_tokens(&self) -> TokenStream2 {
        if self.arg_ty.is_some() {
            quote! { req }
        } else {
            quote! { () }
        }
    }
}
