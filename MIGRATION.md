# Migrating to the function-first API

The trait-based macros (`#[restate_sdk::service]` / `#[object]` / `#[workflow]`) are **deprecated but
still compile**, so you can migrate one service at a time — old and new services can coexist in the
same endpoint.

The migration is mostly mechanical: delete the trait + impl boilerplate, move the context onto each
handler's signature, and wrap the handlers in a `service!` / `object!` / `workflow!` declaration.

---

## Service

**Before**

```rust
#[restate_sdk::service]
trait Greeter {
    async fn greet(name: String) -> HandlerResult<String>;
}

struct GreeterImpl;
impl Greeter for GreeterImpl {
    async fn greet(&self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
        Ok(format!("Hi {name}"))
    }
}

Endpoint::builder().bind(GreeterImpl.serve()).build()
```

**After**

```rust
#[restate_sdk::handler]
async fn greet(_ctx: Context<'_>, name: String) -> HandlerResult<String> {
    Ok(format!("Hi {name}"))
}

service!(Greeter: { greet });

Endpoint::builder().bind(Greeter).build()
```

- The handler is a free `async fn`. The context is a normal parameter (no injected `&self`).
- `service!(Greeter: { greet })` defines the bindable `Greeter` type **and** the `GreeterClient`.
- `bind(GreeterImpl.serve())` → `bind(Greeter)`.

## Virtual Object

The service kind and whether a handler is shared are **inferred from the context type** — drop
`#[shared]`.

**Before**

```rust
#[restate_sdk::object]
trait Counter {
    async fn add(val: u64) -> HandlerResult<u64>;
    #[shared]
    async fn get() -> HandlerResult<u64>;
}
```

**After**

```rust
#[restate_sdk::handler]
async fn add(ctx: ObjectContext<'_>, val: u64) -> HandlerResult<u64> { /* ... */ }

#[restate_sdk::handler]
async fn get(ctx: SharedObjectContext<'_>) -> HandlerResult<u64> { /* ... */ }

object!(Counter: { add, get });
```

| Context type in the signature | Kind | Shared? |
|---|---|---|
| `Context` | service | — |
| `ObjectContext` | virtual object | exclusive |
| `SharedObjectContext` | virtual object | shared |
| `WorkflowContext` | workflow | `run` |
| `SharedWorkflowContext` | workflow | shared |

## Workflow

```rust
#[restate_sdk::handler]
async fn run(ctx: WorkflowContext<'_>, input: String) -> HandlerResult<String> { /* ... */ }

#[restate_sdk::handler]
async fn signal(ctx: SharedWorkflowContext<'_>) -> HandlerResult<()> { /* ... */ }

workflow!(MyWorkflow: { run, signal });
```

## Dependencies (HTTP clients, pools, ...): struct fields → extensions

Dependencies used to live on the impl struct and were read via `&self`. Now they are **extensions**,
attached at bind time and read from the context.

**Before**

```rust
struct GreeterImpl { client: reqwest::Client }
impl Greeter for GreeterImpl {
    async fn greet(&self, ctx: Context<'_>, url: String) -> HandlerResult<String> {
        let client = self.client.clone();
        /* ... */
    }
}

Endpoint::builder().bind(GreeterImpl { client }.serve()).build()
```

**After**

```rust
#[restate_sdk::handler]
async fn greet(ctx: Context<'_>, url: String) -> HandlerResult<String> {
    let client = ctx.extension::<reqwest::Client>().clone();
    /* ... */
}

service!(Greeter: { greet });

// Service-scoped (overrides an endpoint-level extension of the same type):
Endpoint::builder().bind(Greeter.extension(reqwest::Client::new())).build()
// ...or endpoint-wide, shared by every service:
Endpoint::builder().extension(reqwest::Client::new()).bind(Greeter).build()
```

Read them with `ctx.extension::<T>()` (panics if missing) or `ctx.try_extension::<T>()`.

## Options

`bind_with_options(..)` is deprecated; attach options to the definition and `bind` it.

```rust
// Before
Endpoint::builder()
    .bind_with_options(GreeterImpl.serve(), ServiceOptions::new().journal_retention(dur))
    .build()

// After
Endpoint::builder()
    .bind(Greeter.options(ServiceOptions::new().journal_retention(dur)))
    .build()
```

## Names

**Handler name:** `#[name = "..."]` on the method → `#[restate_sdk::handler(name = "...")]`.

```rust
// Before
#[restate_sdk::service]
trait Greeter {
    #[name = "greetHandler"]
    async fn greet() -> HandlerResult<()>;
}

// After
#[restate_sdk::handler(name = "greetHandler")]
async fn greet(_ctx: Context<'_>) -> HandlerResult<()> { Ok(()) }

service!(Greeter: { greet });
```

**Service name:** the identifier in `service!(Name: { .. })` *is* the wire name, so it must be a
valid Rust identifier (typically the same `CamelCase` as before). There is **no service-name
override yet**: if you relied on `#[name = "..."]` on the trait to give a service a wire name that
isn't a valid identifier (e.g. it contains `-` or `.`), keep that service on the deprecated trait
macro for now.

Invalid names are now a **compile error** (they used to panic when the discovery manifest was built).

## Calling other services

`service!`/`object!`/`workflow!` still generate `{Name}Client`, so the caller side is unchanged:

```rust
let greeting = ctx.service_client::<GreeterClient>().greet(name).call().await?;
```

One wart: the generated client method always takes the input argument, so a **no-input** handler is
called with `()`:

```rust
ctx.object_client::<CounterClient>(key).get(()).call().await?; // note the `(())`
```

## Reusing a handler's body from another handler

`#[handler]` keeps the body callable as `name::call(ctx, input)`:

```rust
#[restate_sdk::handler]
async fn add(ctx: ObjectContext<'_>, val: u64) -> HandlerResult<u64> { /* ... */ }

#[restate_sdk::handler]
async fn increment(ctx: ObjectContext<'_>) -> HandlerResult<u64> {
    add::call(ctx, 1).await
}
```

## Cheat sheet

| Old | New |
|---|---|
| `#[restate_sdk::service] trait S { async fn h(..) }` + `impl S for I` | `#[restate_sdk::handler] async fn h(ctx, ..)` + `service!(S: { h })` |
| `#[object]` / `#[workflow]` trait | `object!(..)` / `workflow!(..)` |
| `#[shared]` on a method | use a `Shared*Context` in the signature |
| context injected as `&self, ctx` | context is the first fn parameter |
| dependency on the impl struct (`self.x`) | `ctx.extension::<X>()` + `.extension(x)` at bind |
| `impl.serve()` | the `service!` type (e.g. `Greeter`) |
| `bind_with_options(x.serve(), o)` | `bind(X.options(o))` |
| `#[name = ".."]` on a method | `#[restate_sdk::handler(name = "..")]` |
| `FooClient` (from the trait) | `FooClient` (from `service!`), no-input calls take `()` |
