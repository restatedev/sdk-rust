# Release Notes for PR #129: Scoped ingress clients and workflow handle lookup

## New Feature

### What Changed

Generated ingress clients gain two additions:

- **`scoped_client(...)`** — an alternative constructor (alongside `from_client`) that binds the
  client to a Restate Cloud scope. Every request issued by a scoped client (and its workflow
  `handle()` lookup) is routed through the `/restate/scope/{scope}/...` prefix.
- **Workflow `handle()`** — generated workflow ingress clients expose an async `handle()` method
  that resolves the [`InvocationHandle`] for the workflow invocation keyed by the client's key. The
  handle is typed to the workflow's `run` handler output, so `attach()`/`output()` decode that
  result. If a workflow declares a handler literally named `handle`, its generated method is renamed
  to `_handle` so the injected lookup keeps the plain `handle` name (the on-the-wire handler name is
  unchanged).

At the transport level, `Client::lookup_workflow(workflow_name, workflow_key, scope)` is now public.
It resolves a workflow invocation to a typed `InvocationHandle` through the `/restate/lookup` route.

### Why This Matters

- Restate Cloud users can target a specific scope from a single `Client` without threading a
  `.scope(...)` call through every request.
- Workflows can be re-attached from outside a Restate service by key alone, without having to
  persist the invocation ID returned at submission time.

### Impact on Users

- No breaking changes; these are purely additive APIs.
- `handle()` and `lookup_workflow` require Restate >= 1.7 (the `/restate/lookup` route).
- Scoped routing requires Restate Cloud.

### Example

```rust
use restate_sdk::ingress::ReqwestClient;

let client = ReqwestClient::connect("http://localhost:8080".parse()?)?;

// Submit a workflow, then re-attach to it later by key.
let workflow = MyWorkflowIngressClient::from_client(client.clone(), "order-42");
workflow.run("input".to_owned()).send().await?;

let handle = workflow.handle().await?;
let result = handle.attach().await?.into_body();

// Scoped client: every request runs in the given Restate Cloud scope.
let scoped = MyWorkflowIngressClient::scoped_client(client, "order-42", "prod");
```

### Related Issues

- PR #129
