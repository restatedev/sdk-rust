//! # Configuring services and handlers
//!
//! Services, virtual objects and workflows — and their individual handlers — are configured through
//! arguments on the [`#[service]`](macro@crate::service) / [`#[object]`](macro@crate::object) /
//! [`#[workflow]`](macro@crate::workflow) and [`#[handler]`](macro@crate::handler) attributes.
//!
//! Options set on the **service** attribute act as defaults for every handler; the same option set
//! on a **`#[handler]`** overrides the service-wide default for that one handler.
//!
//! ```rust,no_run
//! use restate_sdk::prelude::*;
//!
//! struct MyService;
//!
//! #[restate_sdk::service(
//!     // Service-wide defaults, applied to every handler unless it overrides them.
//!     inactivity_timeout = "10m",
//!     abort_timeout = "1m",
//!     idempotency_retention = "1 day",
//!     invocation_retry_policy(
//!         initial_interval = "100ms",
//!         factor = 2.0,
//!         max_interval = "3s",
//!         max_attempts = 10,
//!         on_max_attempts = "pause",
//!     ),
//! )]
//! impl MyService {
//!     /// This doc comment becomes the handler's `documentation` in discovery
//!     /// (surfaced in the Restate UI and the OpenAPI export).
//!     #[handler(
//!         // Overrides, only for this handler.
//!         inactivity_timeout = "30s",
//!         journal_retention = "7 days",
//!     )]
//!     async fn my_handler(&self, _ctx: Context<'_>) -> Result<(), HandlerError> {
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Durations
//!
//! Every duration-valued option is a string parsed with [jiff]'s "friendly" format, e.g. `"500ms"`,
//! `"30s"`, `"5m"`, `"2h"`, `"1 day"`, or `"1m 30s"`.
//!
//! ## Options
//!
//! Available on **both** the service attribute (as defaults) and `#[handler]` (as overrides):
//!
//! | Option | Type | Description |
//! |---|---|---|
//! | `name` | string | Override the service / handler name registered in Restate. |
//! | `inactivity_timeout` | duration | How long an invocation may stay in-flight without making progress before Restate asks it to suspend gracefully. |
//! | `abort_timeout` | duration | How long Restate waits after the inactivity timeout before forcibly aborting the invocation (killing the handler task). |
//! | `journal_retention` | duration | How long the invocation journal is retained after completion. |
//! | `idempotency_retention` | duration | How long the result of an idempotent invocation is retained for deduplication. |
//! | `lazy_state` | bool | Load virtual-object K/V state lazily. Bare `lazy_state` is shorthand for `lazy_state = true`. |
//! | `ingress_private` | bool | Make the service / handler private, i.e. only callable from other handlers, not directly from ingress. |
//! | [`invocation_retry_policy(..)`](#invocation-retry-policy) | group | The invocation retry policy — see below. |
//!
//! Two options are attribute-specific:
//!
//! * `client_visibility = "pub(crate)"` — service attribute only; sets the visibility of the
//!   generated client (defaults to `pub`).
//! * `workflow_completion_retention` — `#[workflow]` attribute only (see
//!   [below](#workflow-completion-retention)).
//!
//! ## Invocation retry policy
//!
//! `invocation_retry_policy(..)` groups the retry settings for an invocation. Every field is
//! optional; unset fields fall back to the Restate server defaults (or the service-wide policy, when
//! set on a handler).
//!
//! ```rust,no_run
//! use restate_sdk::prelude::*;
//! # struct MyService;
//! #[restate_sdk::service]
//! impl MyService {
//!     #[handler(invocation_retry_policy(
//!         initial_interval = "100ms", // delay before the first retry
//!         factor = 2.0,               // multiplier applied to the interval after each attempt
//!         max_interval = "10s",       // upper bound on the retry interval
//!         max_attempts = 10,          // give up after this many attempts
//!         on_max_attempts = "kill",   // "pause" or "kill" the invocation once max_attempts is hit
//!     ))]
//!     async fn my_handler(&self, _ctx: Context<'_>) -> Result<(), HandlerError> {
//!         Ok(())
//!     }
//! }
//! ```
//!
//! `on_max_attempts` accepts `"pause"` (suspend the invocation so it can be resumed later) or
//! `"kill"` (fail it permanently). `factor` accepts an integer or a float.
//!
//! ## Workflow completion retention
//!
//! For workflows, configure how long the completed workflow's result, K/V state and promises are
//! retained after the `run` handler finishes with `workflow_completion_retention` — set on the
//! `#[workflow]` attribute, since a workflow has a single `run` handler:
//!
//! ```rust,no_run
//! use restate_sdk::prelude::*;
//! # struct MyWorkflow;
//! #[restate_sdk::workflow(workflow_completion_retention = "3 days")]
//! impl MyWorkflow {
//!     #[handler]
//!     async fn run(&self, _ctx: WorkflowContext<'_>) -> Result<(), HandlerError> {
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Configuring in code
//!
//! The same options can be applied programmatically at bind time via
//! [`ServiceDefinition::options`](crate::service::ServiceDefinition::options), which takes a
//! [`ServiceOptions`](crate::endpoint::ServiceOptions) (with per-handler
//! [`HandlerOptions`](crate::endpoint::HandlerOptions)) — useful when the values come from your own
//! configuration rather than being known at compile time. Durations are plain
//! [`Duration`](std::time::Duration)s, and `on_max_attempts` is chosen via
//! `retry_policy_pause_on_max_attempts()` / `retry_policy_kill_on_max_attempts()`.
//!
//! ```rust,no_run
//! use restate_sdk::prelude::*;
//! use std::time::Duration;
//! # struct MyService;
//! # #[restate_sdk::service]
//! # impl MyService {
//! #     #[handler]
//! #     async fn my_handler(&self, _ctx: Context<'_>) -> Result<(), HandlerError> { Ok(()) }
//! # }
//! let service = MyService
//!     .into_service_definition()
//!     .options(
//!         ServiceOptions::new()
//!             .inactivity_timeout(Duration::from_secs(30))
//!             .journal_retention(Duration::from_secs(60 * 60 * 24))
//!             .retry_policy_initial_interval(Duration::from_millis(100))
//!             .retry_policy_max_attempts(10)
//!             .retry_policy_pause_on_max_attempts()
//!             // override options for a single handler
//!             .handler(
//!                 "my_handler",
//!                 HandlerOptions::new().inactivity_timeout(Duration::from_secs(5)),
//!             ),
//!     );
//!
//! let endpoint = Endpoint::builder().bind(service).build();
//! ```
//!
//! [jiff]: https://docs.rs/jiff
