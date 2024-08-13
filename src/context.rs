use crate::endpoint::{ContextInternal, InputMetadata};
use crate::errors::{HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::time::Duration;

pub struct Context<'a>(&'a ContextInternal);

impl<'a> From<(&'a ContextInternal, InputMetadata)> for Context<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self(value.0)
    }
}

pub struct SharedObjectContext<'a>(pub(crate) &'a ContextInternal);

impl<'a> From<(&'a ContextInternal, InputMetadata)> for SharedObjectContext<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self(value.0)
    }
}

pub struct ObjectContext<'a>(pub(crate) &'a ContextInternal);

impl<'a> From<(&'a ContextInternal, InputMetadata)> for ObjectContext<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self(value.0)
    }
}

pub struct SharedWorkflowContext<'a>(pub(crate) &'a ContextInternal);

impl<'a> From<(&'a ContextInternal, InputMetadata)> for SharedWorkflowContext<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self(value.0)
    }
}

pub struct WorkflowContext<'a>(pub(crate) &'a ContextInternal);

impl<'a> From<(&'a ContextInternal, InputMetadata)> for WorkflowContext<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self(value.0)
    }
}

// Little macro to simplify implementing context methods on all the context structs
macro_rules! impl_context_method {
    ([$ctx:ident, $($morectx:ident),*]; $($sig:tt)*) => {
        impl_context_method!(@render_impl $ctx; $($sig)*);
        impl_context_method!([$($morectx),*]; $($sig)*);
    };
    ([$ctx:ident]; $($sig:tt)*) => {
        impl_context_method!(@render_impl $ctx; $($sig)*);
    };
    (@render_impl $ctx:ident; #[doc = $doc:expr] async fn $name:ident $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)? ($($param:ident : $ty:ty),*) -> $ret:ty $(where $( $wlt:tt $( : $wclt:tt $(+ $wdlt:tt )* )? ),+ )?) => {
       impl<'a> $ctx<'a> {
           #[doc = $doc]
           pub fn $name $(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? (&self, $($param: $ty),*) -> impl Future<Output=$ret> + 'a {
               self.0.$name($($param),*)
           }
       }
    };
    (@render_impl $ctx:ident; #[doc = $doc:expr] fn $name:ident $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)? ($($param:ident : $ty:ty),*) -> $ret:ty $(where $( $wlt:tt $( : $wclt:tt $(+ $wdlt:tt )* )? ),+ )?) => {
       impl<'a> $ctx<'a> {
           #[doc = $doc]
           pub fn $name $(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? (&self, $($param: $ty),*) -> $ret {
               self.0.$name($($param),*)
           }
       }
    };
}

// State read methods
impl_context_method!(
    [SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Get state
    async fn get<T: Deserialize + 'static>(key: &str) -> Result<Option<T>, TerminalError>
);
impl_context_method!(
    [SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Get state
    async fn get_keys() -> Result<Vec<String>, TerminalError>
);

// State write methods
impl_context_method!(
    [ObjectContext, WorkflowContext];
    /// Set state
    fn set<T: Serialize + 'static>(key: &str, t: T) -> ()
);
impl_context_method!(
    [ObjectContext, WorkflowContext];
    /// Clear state
    fn clear(key: &str) -> ()
);
impl_context_method!(
    [ObjectContext, WorkflowContext];
    /// Clear state
    fn clear_all() -> ()
);

// Sleep
impl_context_method!(
    [Context, SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Sleep using Restate
    async fn sleep(duration: Duration) -> Result<(), TerminalError>
);

// Calls
#[derive(Debug, Clone)]
pub enum RequestTarget {
    Service {
        name: String,
        handler: String,
    },
    Object {
        name: String,
        key: String,
        handler: String,
    },
    Workflow {
        name: String,
        key: String,
        handler: String,
    },
}

impl RequestTarget {
    pub fn service(name: impl Into<String>, handler: impl Into<String>) -> Self {
        Self::Service {
            name: name.into(),
            handler: handler.into(),
        }
    }

    pub fn object(
        name: impl Into<String>,
        key: impl Into<String>,
        handler: impl Into<String>,
    ) -> Self {
        Self::Object {
            name: name.into(),
            key: key.into(),
            handler: handler.into(),
        }
    }

    pub fn workflow(
        name: impl Into<String>,
        key: impl Into<String>,
        handler: impl Into<String>,
    ) -> Self {
        Self::Workflow {
            name: name.into(),
            key: key.into(),
            handler: handler.into(),
        }
    }
}

impl fmt::Display for RequestTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestTarget::Service { name, handler } => write!(f, "{name}/{handler}"),
            RequestTarget::Object { name, key, handler } => write!(f, "{name}/{key}/{handler}"),
            RequestTarget::Workflow { name, key, handler } => write!(f, "{name}/{key}/{handler}"),
        }
    }
}

impl_context_method!(
    [Context, SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Call another Restate service
    async fn call<Req: Serialize + 'static, Res: Deserialize + 'static>(request_target: RequestTarget, req: Req) -> Result<Res, TerminalError>
);
impl_context_method!(
    [Context, SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Call another Restate service one way
    fn send<Req: Serialize + 'static>(request_target: RequestTarget, req: Req, delay: Option<Duration>) -> ()
);

// Awakeables
impl_context_method!(
    [Context, SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Create an awakeable
    fn awakeable<T: Deserialize + 'static>() -> (String, impl Future<Output = Result<T, TerminalError>> + Send + Sync)
);
impl_context_method!(
    [Context, SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Resolve an awakeable
    fn resolve_awakeable<T: Serialize + 'static>(key: &str, t: T) -> ()
);
impl_context_method!(
    [Context, SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Resolve an awakeable
    fn reject_awakeable(key: &str, failure: TerminalError) -> ()
);

// Promises
impl_context_method!(
    [SharedWorkflowContext, WorkflowContext];
    /// Create a promise
    async fn promise<T: Deserialize + 'static>(key: &str) -> Result<T, TerminalError>
);
impl_context_method!(
    [SharedWorkflowContext, WorkflowContext];
    /// Peek a promise
    async fn peek_promise<T: Deserialize + 'static>(key: &str) -> Result<Option<T>, TerminalError>
);
impl_context_method!(
    [SharedWorkflowContext, WorkflowContext];
    /// Resolve a promise
    fn resolve_promise<T: Serialize + 'static>(key: &str, t: T) -> ()
);
impl_context_method!(
    [Context, SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext];
    /// Resolve a promise
    fn reject_promise(key: &str, failure: TerminalError) -> ()
);

// Run
pub trait RunClosure {
    type Output: Deserialize + Serialize + 'static;
    type Fut: Future<Output = HandlerResult<Self::Output>>;

    fn run(self) -> Self::Fut;
}

impl<F, O, Fut> RunClosure for F
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = HandlerResult<O>>,
    O: Deserialize + Serialize + 'static,
{
    type Output = O;
    type Fut = Fut;

    fn run(self) -> Self::Fut {
        self()
    }
}

// ad-hoc macro to copy paste run
macro_rules! impl_run_method {
    ([$ctx:ident, $($morectx:ident),*]) => {
        impl_run_method!(@render_impl $ctx);
        impl_run_method!([$($morectx),*]);
    };
    ([$ctx:ident]) => {
        impl_run_method!(@render_impl $ctx);
    };
    (@render_impl $ctx:ident) => {
       impl<'a> $ctx<'a> {
              /// Run a non-deterministic operation
                pub fn run<R, F, T>(
                    &self,
                    name: &'a str,
                    run_closure: R,
                ) -> impl Future<Output = Result<T, TerminalError>> + 'a
                where
                    R: RunClosure<Fut = F, Output = T> + Send + Sync + 'a,
                    T: Serialize + Deserialize,
                    F: Future<Output = HandlerResult<T>> + Send + Sync + 'a,
                {
                    self.0.run(name, run_closure)
                }
       }
    };
}

impl_run_method!([Context, SharedObjectContext, ObjectContext, SharedWorkflowContext, WorkflowContext]);
