// Thanks tokio for the help!
// https://github.com/tokio-rs/tokio/blob/a258bff7018940b438e5de3fb846588454df4e4d/tokio/src/macros/select.rs
// MIT License

/// Select macro, alike tokio::select:
///
/// ```rust
/// # use restate_sdk::prelude::*;
/// # use std::convert::Infallible;
/// # use std::time::Duration;
/// #
/// # async fn handle(ctx: Context<'_>) -> Result<(), HandlerError> {
/// # let (_, awakeable) = ctx.awakeable::<String>();
/// # let (_, call_result) = ctx.awakeable::<String>();
/// restate_sdk::select! {
///     // Bind res to the awakeable result
///     res = awakeable => {
///         // Handle awakeable result
///     },
///     _ = ctx.sleep(Duration::from_secs(10)) => {
///         // Handle sleep
///     },
///     // You can also pattern match
///     Ok(success_result) = call_result => {
///         // Handle success result
///     },
///     else => {
///         // Optional: handle cases when pattern matching doesn't match a future result
///         // If unspecified, select panics when there is no match, e.g. in the above select arm,
///         // if call_result returns Err, it would panic unless you specify an else arm.
///     },
///     on_cancel => {
///         // Optional: handle when the invocation gets cancelled during this select.
///         // If unspecified, it just propagates the TerminalError
///     }
/// }
/// #    Ok(())
/// # }
/// ```
///
/// Note: This API is experimental and subject to changes.
#[macro_export]
macro_rules! select {
    // The macro is structured as a tt-muncher. All branches are processed and
    // normalized. Once the input is normalized, it is passed to the top-most
    // rule. When entering the macro, `@{ }` is inserted at the front. This is
    // used to collect the normalized input.
    //
    // The macro only recurses once per branch. This allows using `select!`
    // without requiring the user to increase the recursion limit.

    // All input is normalized, now transform.
    (@ {
        // One `_` for each branch in the `select!` macro. Passing this to
        // `count!` converts $skip to an integer.
        ( $($count:tt)* )

        // Normalized select branches. `( $skip )` is a set of `_` characters.
        // There is one `_` for each select branch **before** this one. Given
        // that all input futures are stored in a tuple, $skip is useful for
        // generating a pattern to reference the future for the current branch.
        // $skip is also used as an argument to `count!`, returning the index of
        // the current select branch.
        $( ( $($skip:tt)* ) $bind:pat = $fut:expr => $handle:expr, )+

        // Expression used to special handle cancellation when awaiting select
        ; $on_cancel:expr

        // Fallback expression used when all select branches have been disabled.
        ; $else:expr
    }) => {{
        use $crate::context::DurableFuture;
        use $crate::context::macro_support::SealedDurableFuture;

        let futures_init = ($( $fut, )+);
        let handles = vec![$(
            $crate::count_field!(futures_init.$($skip)*).handle()
        ,)+];
        let select_fut = futures_init.0.inner_context().select(handles);

        match select_fut.await {
            $(
                Ok($crate::count!( $($skip)* )) => {
                    match $crate::count_field!(futures_init.$($skip)*).await {
                        $bind => {
                            $handle
                        }
                        _ => {
                            $else
                        }
                    }
                }
            )*
            Ok(_) => {
                unreachable!("Select fut returned index out of bounds")
            }
            Err(_) => {
                $on_cancel
            }
            _ => unreachable!("reaching this means there probably is an off by one bug"),
        }
    }};

    // ==== Normalize =====

    // These rules match a single `select!` branch and normalize it for
    // processing by the first rule.

    (@ { $($t:tt)* } ) => {
        // No `else` branch
        $crate::select!(@{ $($t)*; { Err(TerminalError::new_with_code(409, "cancelled"))? }; panic!("No else branch is defined")})
    };
    (@ { $($t:tt)* } on_cancel => $on_cancel:expr $(,)?) => {
        // on_cancel branch
        $crate::select!(@{ $($t)*; $on_cancel; panic!("No else branch is defined") })
    };
    (@ { $($t:tt)* } else => $else:expr $(,)?) => {
        // on_cancel branch
        $crate::select!(@{ $($t)*; { Err(TerminalError::new_with_code(409, "cancelled"))? }; $else })
    };
    (@ { $($t:tt)* } on_cancel => $on_cancel:expr, else => $else:expr $(,)?) => {
        // on_cancel branch
        $crate::select!(@{ $($t)*; $on_cancel; $else })
    };
    (@ { $($t:tt)* } else => $else:expr, on_cancel => $on_cancel:expr $(,)?) => {
        // on_cancel branch
        $crate::select!(@{ $($t)*; $on_cancel; $else })
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr => $h:block, $($r:tt)* ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f => $h, } $($r)*)
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr => $h:expr, $($r:tt)* ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f => $h, } $($r)*)
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr => $h:block ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f => $h, })
    };
    (@ { ( $($s:tt)* ) $($t:tt)* } $p:pat = $f:expr => $h:expr ) => {
        $crate::select!(@{ ($($s)* _) $($t)* ($($s)*) $p = $f => $h, })
    };

    // ===== Entry point =====

    (on_cancel => $on_cancel:expr $(,)? ) => {{
         compile_error!("select! cannot contain only on_cancel branch.")
    }};
    (else => $else:expr $(,)? ) => {{
         compile_error!("select! cannot contain only else branch.")
    }};

    ( $p:pat = $($t:tt)* ) => {
        $crate::select!(@{ () } $p = $($t)*)
    };

    () => {
        compile_error!("select! requires at least one branch.")
    };
}

// And here... we manually list out matches for up to 64 branches... I'm not
// happy about it either, but this is how we manage to use a declarative macro!

#[macro_export]
#[doc(hidden)]
macro_rules! count {
    () => {
        0
    };
    (_) => {
        1
    };
    (_ _) => {
        2
    };
    (_ _ _) => {
        3
    };
    (_ _ _ _) => {
        4
    };
    (_ _ _ _ _) => {
        5
    };
    (_ _ _ _ _ _) => {
        6
    };
    (_ _ _ _ _ _ _) => {
        7
    };
    (_ _ _ _ _ _ _ _) => {
        8
    };
    (_ _ _ _ _ _ _ _ _) => {
        9
    };
    (_ _ _ _ _ _ _ _ _ _) => {
        10
    };
    (_ _ _ _ _ _ _ _ _ _ _) => {
        11
    };
    (_ _ _ _ _ _ _ _ _ _ _ _) => {
        12
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _) => {
        13
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        14
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        15
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        16
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        17
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        18
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        19
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        20
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        21
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        22
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        23
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        24
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        25
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        26
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        27
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        28
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        29
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        30
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        31
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        32
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        33
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        34
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        35
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        36
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        37
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        38
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        39
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        40
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        41
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        42
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        43
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        44
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        45
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        46
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        47
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        48
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        49
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        50
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        51
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        52
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        53
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        54
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        55
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        56
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        57
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        58
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        59
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        60
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        61
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        62
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        63
    };
    (_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        64
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! count_field {
    ($var:ident. ) => {
        $var.0
    };
    ($var:ident. _) => {
        $var.1
    };
    ($var:ident. _ _) => {
        $var.2
    };
    ($var:ident. _ _ _) => {
        $var.3
    };
    ($var:ident. _ _ _ _) => {
        $var.4
    };
    ($var:ident. _ _ _ _ _) => {
        $var.5
    };
    ($var:ident. _ _ _ _ _ _) => {
        $var.6
    };
    ($var:ident. _ _ _ _ _ _ _) => {
        $var.7
    };
    ($var:ident. _ _ _ _ _ _ _ _) => {
        $var.8
    };
    ($var:ident. _ _ _ _ _ _ _ _ _) => {
        $var.9
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _) => {
        $var.10
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _) => {
        $var.11
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.12
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.13
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.14
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.15
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.16
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.17
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.18
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.19
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.20
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.21
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.22
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.23
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.24
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.25
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.26
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.27
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.28
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.29
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.30
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.31
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.32
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.33
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.34
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.35
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.36
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.37
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.38
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.39
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.40
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.41
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.42
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.43
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.44
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.45
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.46
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.47
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.48
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.49
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.50
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.51
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.52
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.53
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.54
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.55
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.56
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.57
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.58
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.59
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.60
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.61
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.62
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.63
    };
    ($var:ident. _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _) => {
        $var.64
    };
}
