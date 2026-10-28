# Release notes for issue 134: Concurrent journaled actions

## New feature and bug fixes

### What changed

`ctx.run(...)` now returns the configurable `Run` builder. Call `.start()` after `.name()` and `.retry_policy()` to register an owned action immediately and obtain a durable result future. The SDK drives concurrent closures and input through shared VM progress.

### Why this matters

Registering every concurrent action before awaiting prevents journal mismatch during partial replay. Shared progress handles proposal-only waits, sibling wakeups, cancellation, and EOF.

### Impact on users

Existing sequential `.await` and borrowing closures remain supported. Started closures and their futures must own their captures. Dropping the result does not cancel its operation; ending the invocation drops pending closures. Cancellation settles pending results, preserves acknowledged values, and permits new cleanup operations.

### Migration guidance

Register actions in deterministic order before awaiting any:

```rust
let first = ctx.run(|| async { Ok(1u32) }).name("first").start();
let second = ctx.run(|| async { Ok(2u32) }).name("second").start();
let (first, second) = futures::join!(first, second);
let values = (first?, second?);
```

Use owned captures for `.start()`, or keep immediate `.await` for borrowing actions. Use durable selection for races whose result controls later context calls. The minimum Rust version is unchanged.

### Related issues

- #134
- #72
- #89
