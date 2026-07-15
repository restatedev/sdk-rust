// Copyright (c) 2023 -  Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! Compile-time validation of Restate service/handler names.
//!
//! Mirrors the discovery manifest schema patterns for `serviceName`/`handlerName` so that an
//! invalid name (from a `#[handler(name = "...")]` / `#[name = "..."]` override, or an unusual
//! function/trait identifier) is a compile error instead of a `.expect()` panic when the discovery
//! manifest is built.
//!
//! - `handlerName`: `^([a-zA-Z]|_[a-zA-Z0-9])[a-zA-Z0-9_]*$`
//! - `serviceName`: `^([a-zA-Z]|_[a-zA-Z0-9])[a-zA-Z0-9._-]*$`

use proc_macro2::Span;
use syn::Error;

/// Validate a handler name against the `handlerName` schema pattern.
pub(crate) fn validate_handler_name(name: &str, span: Span) -> syn::Result<()> {
    validate(name, span, "handler", |c| {
        c.is_ascii_alphanumeric() || c == '_'
    })
}

/// Validate a service name against the `serviceName` schema pattern (also allows `.` and `-`).
pub(crate) fn validate_service_name(name: &str, span: Span) -> syn::Result<()> {
    validate(name, span, "service", |c| {
        c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
    })
}

/// Both patterns share the same start rule — a letter, or `_` followed by a letter/digit — and
/// differ only in which characters the tail allows (`tail_ok`).
fn validate(name: &str, span: Span, kind: &str, tail_ok: impl Fn(char) -> bool) -> syn::Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(Error::new(span, format!("{kind} name must not be empty")));
    };
    let first_ok = first.is_ascii_alphabetic()
        || (first == '_'
            && chars
                .clone()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric()));
    if !first_ok {
        return Err(Error::new(
            span,
            format!(
                "invalid {kind} name {name:?}: must start with an ASCII letter, or `_` followed by \
                 a letter or digit"
            ),
        ));
    }
    if let Some(bad) = name.chars().find(|c| !tail_ok(*c)) {
        return Err(Error::new(
            span,
            format!("invalid {kind} name {name:?}: character {bad:?} is not allowed"),
        ));
    }
    Ok(())
}
