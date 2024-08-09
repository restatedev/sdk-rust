use crate::endpoint::ContextInternal;
use crate::errors::TerminalError;
use std::future::Future;
use std::time::Duration;

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

// TODO generate those methods with a fancy macro?!

impl<'a> Context<'a> {
    pub fn sleep(
        &self,
        duration: Duration,
    ) -> impl Future<Output = Result<(), TerminalError>> + 'a {
        self.0.sleep(duration)
    }
}

impl<'a> SharedObjectContext<'a> {
    pub fn sleep(
        &self,
        duration: Duration,
    ) -> impl Future<Output = Result<(), TerminalError>> + 'a {
        self.0.sleep(duration)
    }

    pub fn get<T: crate::serde::Deserialize + 'static>(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + 'a {
        self.0.get_state(&key)
    }
}

impl<'a> ObjectContext<'a> {
    pub fn sleep(
        &self,
        duration: Duration,
    ) -> impl Future<Output = Result<(), TerminalError>> + 'a {
        self.0.sleep(duration)
    }

    pub fn get<T: crate::serde::Deserialize+ 'static>(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + 'a {
        self.0.get_state(&key)
    }
}

impl<'a> SharedWorkflowContext<'a> {
    pub fn sleep(
        &self,
        duration: Duration,
    ) -> impl Future<Output = Result<(), TerminalError>> + 'a {
        self.0.sleep(duration)
    }

    pub fn get<T: crate::serde::Deserialize+ 'static>(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + 'a {
        self.0.get_state(&key)
    }
}

impl<'a> WorkflowContext<'a> {
    pub fn sleep(
        &self,
        duration: Duration,
    ) -> impl Future<Output = Result<(), TerminalError>> + 'a {
        self.0.sleep(duration)
    }

    pub fn get<T: crate::serde::Deserialize+ 'static>(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + 'a {
        self.0.get_state(&key)
    }
}
