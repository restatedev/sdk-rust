use crate::endpoint::ContextInternal;
use crate::errors::TerminalError;
use std::future::Future;
use std::time::Duration;
use crate::serde::{Serialize, Deserialize};

// TODO maybe use delegate macro

pub struct Context<'a>(&'a ContextInternal);

impl<'a> From<&'a ContextInternal> for Context<'a> {
    fn from(value: &'a ContextInternal) -> Self {
        Self(value)
    }
}

pub struct SharedObjectContext<'a>(pub(crate) &'a ContextInternal);

impl<'a> From<&'a ContextInternal> for SharedObjectContext<'a> {
    fn from(value: &'a ContextInternal) -> Self {
        Self(value)
    }
}

pub struct ObjectContext<'a>(pub(crate) &'a ContextInternal);

impl<'a> From<&'a ContextInternal> for crate::context::ObjectContext<'a> {
    fn from(value: &'a ContextInternal) -> Self {
        Self(value)
    }
}

pub struct SharedWorkflowContext<'a>(pub(crate) &'a ContextInternal);

impl<'a> From<&'a ContextInternal> for crate::context::SharedWorkflowContext<'a> {
    fn from(value: &'a ContextInternal) -> Self {
        Self(value)
    }
}

pub struct WorkflowContext<'a>(pub(crate) &'a ContextInternal);

impl<'a> From<&'a ContextInternal> for crate::context::WorkflowContext<'a> {
    fn from(value: &'a ContextInternal) -> Self {
        Self(value)
    }
}

macro_rules! impl_context_method {
    ([$ctx:ident, $($morectx:ident),*]; $($sig:tt)*) => {
        impl_context_method!(@render_impl $ctx; $($sig)*);
        impl_context_method!([$($morectx),*]; $($sig)*);
    };
    ([$ctx:ident]; $($sig:tt)*) => {
        impl_context_method!(@render_impl $ctx; $($sig)*);
    };
    (@render_impl $ctx:ident; #[doc = $doc:expr] async fn $name:ident $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)? ($($param:ident : $ty:ty),*) -> $ret:ty) => {
       impl<'a> $ctx<'a> {
           #[doc = $doc]
           pub fn $name $(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? (&self, $($param: $ty),*) -> impl Future<Output=$ret> + 'a {
               self.0.$name($($param),*)
           }
       }
    };
    (@render_impl $ctx:ident; #[doc = $doc:expr] fn $name:ident $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)? ($($param:ident : $ty:ty),*) -> $ret:ty) => {
       impl<'a> $ctx<'a> {
           #[doc = $doc]
           pub fn $name $(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? (&self, $($param: $ty),*) -> $ret {
               self.0.$name($($param),*)
           }
       }
    };
}

impl_context_method!(
    [Context, SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Sleep using Restate
    async fn sleep(duration: Duration) -> Result<(), TerminalError>
);

impl_context_method!(
    [SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Get state
    async fn get<T: Deserialize + 'static>(key: &str) -> Result<Option<T>, TerminalError>
);

impl_context_method!(
    [ObjectContext, WorkflowContext];
    /// Get state
    fn set<T: Serialize + 'static>(key: &str, t: T) -> ()
);
