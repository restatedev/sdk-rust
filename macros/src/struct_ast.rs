// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Parsing for the struct-based service API: `#[restate_sdk::service]` (and `#[object]`/`#[workflow]`)
//! applied to an inherent `impl` block, with handlers annotated `#[handler]`.

use crate::ast::{ServiceType, extract_handler_result_parameter};
use syn::punctuated::Punctuated;
use syn::token::Comma;
use syn::{
    Attribute, Error, Expr, ExprLit, FnArg, Ident, ImplItem, ItemImpl, Lit, Meta, MetaNameValue,
    PatType, Result, ReturnType, Type, Visibility, parse_quote,
};

/// Configuration options accepted both by the service/object/workflow attribute (as service-wide
/// defaults) and by the `#[handler]` attribute (as per-handler overrides). Durations are parsed
/// with `jiff` and converted to milliseconds at macro-expansion time, so codegen only has to splice
/// integers. Every field is optional; unset fields become `None` in the generated discovery.
#[derive(Default)]
pub(crate) struct CommonOptions {
    pub(crate) inactivity_timeout: Option<u64>,
    pub(crate) abort_timeout: Option<u64>,
    pub(crate) journal_retention: Option<u64>,
    pub(crate) idempotency_retention: Option<u64>,
    /// Handler-only; rejected on the service attribute (no such field on `discovery::Service`).
    pub(crate) workflow_completion_retention: Option<u64>,
    pub(crate) enable_lazy_state: Option<bool>,
    pub(crate) ingress_private: Option<bool>,
    pub(crate) retry_policy: RetryPolicyArgs,
}

/// Parsed `invocation_retry_policy(..)` group.
#[derive(Default)]
pub(crate) struct RetryPolicyArgs {
    pub(crate) initial_interval: Option<u64>,
    pub(crate) max_interval: Option<u64>,
    pub(crate) factor: Option<f64>,
    pub(crate) max_attempts: Option<u64>,
    pub(crate) on_max_attempts: Option<OnMaxAttempts>,
}

#[derive(Clone, Copy)]
pub(crate) enum OnMaxAttempts {
    Pause,
    Kill,
}

/// Where a `CommonOptions` set is being parsed, used to gate level-specific options. The service
/// level carries the service kind so `workflow_completion_retention` can be restricted to workflows.
#[derive(Clone, Copy)]
enum ConfigLevel {
    Service(ServiceType),
    Handler,
}

/// Arguments accepted by the service/object/workflow attribute in the struct API,
/// e.g. `#[restate_sdk::service(name = "Greeter", client_visibility = "pub(crate)")]` plus any of
/// the shared [`CommonOptions`].
#[derive(Default)]
pub(crate) struct ServiceArgs {
    pub(crate) name: Option<String>,
    pub(crate) vis: Option<Visibility>,
    pub(crate) options: CommonOptions,
}

/// Parse the service/object/workflow attribute arguments. Takes the service kind so that
/// `workflow_completion_retention` can be accepted only on `#[workflow]`.
pub(crate) fn parse_service_args(
    tokens: proc_macro2::TokenStream,
    service_ty: ServiceType,
) -> Result<ServiceArgs> {
    use syn::parse::Parser;

    let mut args = ServiceArgs::default();
    if tokens.is_empty() {
        return Ok(args);
    }
    let metas = Punctuated::<Meta, Comma>::parse_terminated.parse2(tokens)?;
    for meta in &metas {
        if apply_common_meta(&mut args.options, meta, ConfigLevel::Service(service_ty))? {
            continue;
        }
        let ident = meta.path().require_ident()?;
        if ident == "name" {
            args.name = Some(parse_str_lit(name_value_expr(meta)?)?);
        } else if ident == "client_visibility" {
            let s = parse_str_lit(name_value_expr(meta)?)?;
            args.vis = Some(syn::parse_str::<Visibility>(&s)?);
        } else {
            return Err(Error::new_spanned(
                meta,
                format!(
                    "unsupported attribute argument `{ident}`; supported arguments are: \
                     name, client_visibility, inactivity_timeout, abort_timeout, \
                     journal_retention, idempotency_retention, lazy_state, ingress_private, \
                     invocation_retry_policy(..){}",
                    if service_ty == ServiceType::Workflow {
                        ", workflow_completion_retention"
                    } else {
                        ""
                    }
                ),
            ));
        }
    }
    Ok(args)
}

/// Match one meta item against the shared configuration options. Returns `Ok(true)` if it matched a
/// common option (already applied), `Ok(false)` if unrecognized so the caller can handle its own
/// keys (`name`/`client_visibility`) or raise an error.
fn apply_common_meta(opts: &mut CommonOptions, meta: &Meta, level: ConfigLevel) -> Result<bool> {
    let Some(ident) = meta.path().get_ident() else {
        return Ok(false);
    };
    match ident.to_string().as_str() {
        "inactivity_timeout" => opts.inactivity_timeout = Some(parse_duration_meta(meta)?),
        "abort_timeout" => opts.abort_timeout = Some(parse_duration_meta(meta)?),
        "journal_retention" => opts.journal_retention = Some(parse_duration_meta(meta)?),
        "idempotency_retention" => opts.idempotency_retention = Some(parse_duration_meta(meta)?),
        // Configured at the workflow level (there is a single `run` handler), even though the
        // discovery schema carries it on the workflow handler. Rejected elsewhere.
        "workflow_completion_retention" => match level {
            ConfigLevel::Service(ServiceType::Workflow) => {
                opts.workflow_completion_retention = Some(parse_duration_meta(meta)?);
            }
            ConfigLevel::Service(_) => {
                return Err(Error::new_spanned(
                    meta,
                    "`workflow_completion_retention` is only valid on `#[workflow]`",
                ));
            }
            ConfigLevel::Handler => {
                return Err(Error::new_spanned(
                    meta,
                    "`workflow_completion_retention` is configured on the `#[workflow]` attribute, \
                     not on individual handlers",
                ));
            }
        },
        "lazy_state" => opts.enable_lazy_state = Some(parse_flag_or_bool(meta)?),
        "ingress_private" => opts.ingress_private = Some(parse_flag_or_bool(meta)?),
        "invocation_retry_policy" => parse_retry_policy(&mut opts.retry_policy, meta)?,
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_retry_policy(retry: &mut RetryPolicyArgs, meta: &Meta) -> Result<()> {
    let Meta::List(list) = meta else {
        return Err(Error::new_spanned(
            meta,
            "`invocation_retry_policy` expects a group, e.g. \
             `invocation_retry_policy(max_attempts = 5, initial_interval = \"100ms\")`",
        ));
    };
    let inner = list.parse_args_with(Punctuated::<MetaNameValue, Comma>::parse_terminated)?;
    for nv in &inner {
        let ident = nv.path.require_ident()?;
        match ident.to_string().as_str() {
            "initial_interval" => retry.initial_interval = Some(parse_duration_expr(&nv.value)?),
            "max_interval" => retry.max_interval = Some(parse_duration_expr(&nv.value)?),
            "factor" => retry.factor = Some(parse_f64(&nv.value)?),
            "max_attempts" => retry.max_attempts = Some(parse_u64(&nv.value)?),
            "on_max_attempts" => retry.on_max_attempts = Some(parse_on_max_attempts(&nv.value)?),
            other => {
                return Err(Error::new_spanned(
                    &nv.path,
                    format!(
                        "unknown `invocation_retry_policy` option `{other}`; supported options are: \
                         initial_interval, max_interval, factor, max_attempts, on_max_attempts"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The value expression of a `key = value` meta.
fn name_value_expr(meta: &Meta) -> Result<&Expr> {
    match meta {
        Meta::NameValue(nv) => Ok(&nv.value),
        _ => Err(Error::new_spanned(
            meta,
            "expected `key = value`, e.g. `inactivity_timeout = \"30s\"`",
        )),
    }
}

fn parse_str_lit(expr: &Expr) -> Result<String> {
    Ok(as_str_lit(expr)?.value())
}

fn as_str_lit(expr: &Expr) -> Result<syn::LitStr> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = expr
    {
        Ok(s.clone())
    } else {
        Err(Error::new_spanned(expr, "expected a string literal"))
    }
}

fn parse_duration_meta(meta: &Meta) -> Result<u64> {
    parse_duration_expr(name_value_expr(meta)?)
}

fn parse_duration_expr(expr: &Expr) -> Result<u64> {
    parse_duration_lit(&as_str_lit(expr)?)
}

/// Parse a human duration string (jiff's "friendly" format, e.g. `"1 sec"`, `"30s"`, `"5m"`,
/// `"500ms"`, `"2h"`, `"1 day"`, `"7 days"`) into whole milliseconds.
///
/// We parse into a [`jiff::Span`] (rather than `SignedDuration`) so calendar-ish units up to weeks
/// are accepted, treating days as 24h and weeks as 7 days. Months/years are intentionally rejected
/// by jiff here: they have no fixed millisecond length without a reference date, so they'd be
/// ambiguous for a timeout/retention value.
fn parse_duration_lit(lit: &syn::LitStr) -> Result<u64> {
    let raw = lit.value();
    let span: jiff::Span = raw
        .trim()
        .parse()
        .map_err(|e| Error::new(lit.span(), format!("invalid duration {raw:?}: {e}")))?;
    let ms = span
        .total((
            jiff::Unit::Millisecond,
            jiff::SpanRelativeTo::days_are_24_hours(),
        ))
        .map_err(|e| Error::new(lit.span(), format!("invalid duration {raw:?}: {e}")))?;
    if ms < 0.0 {
        return Err(Error::new(
            lit.span(),
            format!("duration {raw:?} must not be negative"),
        ));
    }
    let ms = ms.round();
    if ms > u64::MAX as f64 {
        return Err(Error::new(
            lit.span(),
            format!("duration {raw:?} is too large to represent in milliseconds"),
        ));
    }
    Ok(ms as u64)
}

fn parse_flag_or_bool(meta: &Meta) -> Result<bool> {
    match meta {
        // Bare flag, e.g. `#[handler(lazy_state)]`.
        Meta::Path(_) => Ok(true),
        Meta::NameValue(nv) => {
            if let Expr::Lit(ExprLit {
                lit: Lit::Bool(b), ..
            }) = &nv.value
            {
                Ok(b.value)
            } else {
                Err(Error::new_spanned(
                    &nv.value,
                    "expected a boolean literal `true` or `false`",
                ))
            }
        }
        Meta::List(_) => Err(Error::new_spanned(
            meta,
            "expected a bare flag or `= true`/`= false`",
        )),
    }
}

fn parse_f64(expr: &Expr) -> Result<f64> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Float(f), ..
        }) => f.base10_parse::<f64>(),
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => Ok(i.base10_parse::<u64>()? as f64),
        _ => Err(Error::new_spanned(expr, "expected a number, e.g. `2.0`")),
    }
}

fn parse_u64(expr: &Expr) -> Result<u64> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Int(i), ..
    }) = expr
    {
        i.base10_parse::<u64>()
    } else {
        Err(Error::new_spanned(expr, "expected an integer, e.g. `10`"))
    }
}

fn parse_on_max_attempts(expr: &Expr) -> Result<OnMaxAttempts> {
    let s = as_str_lit(expr)?;
    match s.value().as_str() {
        "pause" => Ok(OnMaxAttempts::Pause),
        "kill" => Ok(OnMaxAttempts::Kill),
        other => Err(Error::new_spanned(
            expr,
            format!(r#"invalid `on_max_attempts` value {other:?}; expected "pause" or "kill""#),
        )),
    }
}

/// Collect leading `///`/`#[doc = ".."]` comments from an item's attributes into a single string,
/// stripping the conventional leading space and trimming. Returns `None` when there are none.
fn extract_docs(attrs: &[Attribute]) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc")
            && let Meta::NameValue(MetaNameValue {
                value:
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }),
                ..
            }) = &attr.meta
        {
            let raw = s.value();
            lines.push(raw.strip_prefix(' ').unwrap_or(&raw).to_string());
        }
    }
    let joined = lines.join("\n");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) struct StructService {
    pub(crate) service_ty: ServiceType,
    pub(crate) restate_name: String,
    /// Visibility of the generated client (and impls). Defaults to `pub`.
    pub(crate) vis: Visibility,
    /// The `Self` type of the impl block (e.g. `Greeter` or `Repo<D>`).
    pub(crate) self_ty: Box<Type>,
    /// Last path segment of `self_ty` (e.g. `Greeter`), used to name the client.
    pub(crate) self_ident: Ident,
    /// Generics of the impl block (type params, bounds and where-clause), propagated onto every
    /// generated impl and the client.
    pub(crate) generics: syn::Generics,
    /// Doc-comment on the `impl` block, if any (service-level documentation).
    pub(crate) documentation: Option<String>,
    /// Service-wide configuration defaults parsed from the service/object/workflow attribute.
    pub(crate) options: CommonOptions,
    /// The user's impl block with `#[handler]` attributes stripped, re-emitted verbatim.
    pub(crate) stripped_impl: ItemImpl,
    pub(crate) handlers: Vec<StructHandler>,
}

pub(crate) struct StructHandler {
    pub(crate) is_shared: bool,
    pub(crate) restate_name: String,
    pub(crate) ident: Ident,
    pub(crate) arg: Option<PatType>,
    pub(crate) output_ok: Type,
    /// Doc-comment on the handler method, if any.
    pub(crate) documentation: Option<String>,
    /// Per-handler configuration parsed from `#[handler(..)]`.
    pub(crate) options: CommonOptions,
}

impl StructService {
    pub(crate) fn from_impl(
        service_ty: ServiceType,
        args: ServiceArgs,
        mut item_impl: ItemImpl,
    ) -> Result<Self> {
        if let Some((_, path, _)) = &item_impl.trait_ {
            return Err(Error::new_spanned(
                path,
                "the struct-based Restate API must be applied to an inherent impl block \
                 (`impl MyService { .. }`), not a trait impl",
            ));
        }
        for param in &item_impl.generics.params {
            if let syn::GenericParam::Const(c) = param {
                return Err(Error::new_spanned(
                    c,
                    "const generic parameters are not supported on Restate services",
                ));
            }
        }

        let generics = item_impl.generics.clone();
        let self_ty = item_impl.self_ty.clone();
        let self_ident = type_last_ident(&self_ty)?;
        let documentation = extract_docs(&item_impl.attrs);

        let restate_name = args.name.unwrap_or_else(|| self_ident.to_string());
        let vis = args.vis.unwrap_or_else(|| parse_quote!(pub));
        let options = args.options;

        let mut handlers = Vec::new();
        for item in &mut item_impl.items {
            if let ImplItem::Fn(f) = item
                && let Some(idx) = find_handler_attr(&f.attrs)
            {
                let attr = f.attrs.remove(idx);
                let (name_override, handler_options) = read_handler_attr(&attr)?;
                let documentation = extract_docs(&f.attrs);
                handlers.push(parse_handler(
                    service_ty,
                    f,
                    name_override,
                    handler_options,
                    documentation,
                )?);
            }
        }

        if handlers.is_empty() {
            return Err(Error::new_spanned(
                &item_impl,
                "no `#[handler]` methods found; annotate at least one handler method with `#[handler]`",
            ));
        }

        Ok(Self {
            service_ty,
            restate_name,
            vis,
            self_ty,
            self_ident,
            generics,
            documentation,
            options,
            stripped_impl: item_impl,
            handlers,
        })
    }
}

/// Index of the `#[handler]` (or `#[restate_sdk::handler]`) attribute in `attrs`, if present.
fn find_handler_attr(attrs: &[Attribute]) -> Option<usize> {
    attrs.iter().position(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "handler")
    })
}

/// Read the optional `name = ".."` override and any [`CommonOptions`] from `#[handler(..)]`.
fn read_handler_attr(attr: &Attribute) -> Result<(Option<String>, CommonOptions)> {
    let mut name = None;
    let mut options = CommonOptions::default();
    match &attr.meta {
        Meta::Path(_) => {}
        Meta::List(list) => {
            let metas = list.parse_args_with(Punctuated::<Meta, Comma>::parse_terminated)?;
            for meta in &metas {
                if apply_common_meta(&mut options, meta, ConfigLevel::Handler)? {
                    continue;
                }
                let ident = meta.path().require_ident()?;
                if ident == "name" {
                    name = Some(parse_str_lit(name_value_expr(meta)?)?);
                } else {
                    return Err(Error::new_spanned(
                        meta,
                        format!(
                            "unsupported `#[handler]` argument `{ident}`; supported arguments are: \
                             name, inactivity_timeout, abort_timeout, journal_retention, \
                             idempotency_retention, lazy_state, ingress_private, \
                             invocation_retry_policy(..)"
                        ),
                    ));
                }
            }
        }
        Meta::NameValue(_) => {
            return Err(Error::new_spanned(
                attr,
                "invalid `#[handler]` attribute; use `#[handler]` or `#[handler(name = \"..\", ..)]`",
            ));
        }
    }
    Ok((name, options))
}

fn parse_handler(
    service_ty: ServiceType,
    f: &syn::ImplItemFn,
    name_override: Option<String>,
    options: CommonOptions,
    documentation: Option<String>,
) -> Result<StructHandler> {
    let sig = &f.sig;

    if sig.asyncness.is_none() {
        return Err(Error::new_spanned(
            &sig.ident,
            "Restate handlers must be declared `async fn`",
        ));
    }

    let mut inputs = sig.inputs.iter();

    // First argument: `&self`.
    match inputs.next() {
        Some(FnArg::Receiver(recv)) => {
            if recv.reference.is_none() || recv.mutability.is_some() {
                return Err(Error::new_spanned(
                    recv,
                    "Restate handlers must take `&self`; the service value is shared (behind an Arc) \
                     across concurrent invocations, so use interior mutability for mutable state",
                ));
            }
        }
        _ => {
            return Err(Error::new_spanned(
                &sig.ident,
                "Restate handlers must take `&self` as the first argument",
            ));
        }
    }

    // Second argument: the context.
    let ctx_arg = match inputs.next() {
        Some(FnArg::Typed(pt)) => pt,
        _ => {
            return Err(Error::new_spanned(
                &sig.ident,
                "Restate handlers must take a context as the second argument \
                 (Context, ObjectContext, SharedObjectContext, WorkflowContext or SharedWorkflowContext)",
            ));
        }
    };
    let is_shared = validate_context(service_ty, &ctx_arg.ty)?;

    // Optional third argument: the single input. The parameter pattern (which may destructure,
    // e.g. `Json(x): Json<T>`) stays in the user's impl untouched; we only read its type.
    let mut arg = None;
    for extra in inputs {
        match extra {
            FnArg::Typed(pt) => {
                if arg.is_some() {
                    return Err(Error::new_spanned(
                        extra,
                        "Restate handlers support at most one input argument (after the context)",
                    ));
                }
                arg = Some(pt.clone());
            }
            FnArg::Receiver(recv) => {
                return Err(Error::new_spanned(recv, "unexpected `self` argument"));
            }
        }
    }

    // Return type.
    let (output_ok, _output_err) = match &sig.output {
        ReturnType::Default => {
            return Err(Error::new_spanned(
                &sig.ident,
                "handler return type must be `Result<T, E>` or `restate_sdk::prelude::HandlerResult<T>`",
            ));
        }
        ReturnType::Type(_, ty) => extract_handler_result_parameter(ty).ok_or_else(|| {
            Error::new_spanned(
                ty,
                "handler return type must be `Result<T, E>` or `restate_sdk::prelude::HandlerResult<T>`",
            )
        })?,
    };

    Ok(StructHandler {
        is_shared,
        restate_name: name_override.unwrap_or_else(|| sig.ident.to_string()),
        ident: sig.ident.clone(),
        arg,
        output_ok,
        documentation,
        options,
    })
}

/// Validate that the context type matches the declared service kind, returning whether it is a
/// shared (read-only) context.
fn validate_context(service_ty: ServiceType, ty: &Type) -> Result<bool> {
    let ident = type_last_ident(ty)?;
    let (kind, shared) = match ident.to_string().as_str() {
        "Context" => (ServiceType::Service, false),
        "ObjectContext" => (ServiceType::Object, false),
        "SharedObjectContext" => (ServiceType::Object, true),
        "WorkflowContext" => (ServiceType::Workflow, false),
        "SharedWorkflowContext" => (ServiceType::Workflow, true),
        other => {
            return Err(Error::new_spanned(
                ty,
                format!(
                    "unrecognized context type `{other}`; expected one of Context, ObjectContext, \
                     SharedObjectContext, WorkflowContext, SharedWorkflowContext"
                ),
            ));
        }
    };
    if kind != service_ty {
        return Err(Error::new_spanned(
            ty,
            format!(
                "context type `{ident}` does not match the `#[{}]` service kind; expected {}",
                macro_name(service_ty),
                expected_contexts(service_ty),
            ),
        ));
    }
    Ok(shared)
}

fn macro_name(service_ty: ServiceType) -> &'static str {
    match service_ty {
        ServiceType::Service => "service",
        ServiceType::Object => "object",
        ServiceType::Workflow => "workflow",
    }
}

fn expected_contexts(service_ty: ServiceType) -> &'static str {
    match service_ty {
        ServiceType::Service => "Context",
        ServiceType::Object => "ObjectContext or SharedObjectContext",
        ServiceType::Workflow => "WorkflowContext or SharedWorkflowContext",
    }
}

/// The last path segment ident of a type path (e.g. `Greeter` for `Greeter` or `Context` for
/// `Context<'_>`).
fn type_last_ident(ty: &Type) -> Result<Ident> {
    match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|seg| seg.ident.clone())
            .ok_or_else(|| Error::new_spanned(ty, "expected a named type")),
        _ => Err(Error::new_spanned(ty, "expected a named type")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;

    fn ms(s: &str) -> u64 {
        parse_duration_lit(&syn::LitStr::new(s, Span::call_site())).expect("valid duration")
    }

    #[test]
    fn friendly_durations_parse_to_ms() {
        assert_eq!(ms("1 sec"), 1_000);
        assert_eq!(ms("30s"), 30_000);
        assert_eq!(ms("500ms"), 500);
        assert_eq!(ms("5m"), 300_000);
        assert_eq!(ms("1h"), 3_600_000);
        assert_eq!(ms("2h"), 7_200_000);
        assert_eq!(ms("1m 30s"), 90_000);
        assert_eq!(ms("1 day"), 86_400_000);
        assert_eq!(ms("7 days"), 604_800_000);
    }

    #[test]
    fn invalid_duration_is_rejected() {
        assert!(
            parse_duration_lit(&syn::LitStr::new("not-a-duration", Span::call_site())).is_err()
        );
        // Months/years have no fixed ms length without a reference date -> rejected.
        assert!(parse_duration_lit(&syn::LitStr::new("1 month", Span::call_site())).is_err());
    }
}
