# Logging

`MISSING_EBOOKS_LOG` sets verbosity to one of `error`, `warn`, `info` (the default), `debug`, or `trace`. Raising it to `debug` or `trace` is scoped to this app, so the dependencies stay quiet. Lowering it to `warn` or `error` quiets everything to that level.

## Per-operation timings at `debug`

`debug` adds per-operation timings: per-root scans, cache hits and misses, marker writes, and request and render latency. Run at this level when checking how a real library performs, because these are the numbers the network-shares page refers to.

`trace` adds a line per directory walked. Useful when a specific folder is behaving oddly, noisy for anything else.

## `RUST_LOG` override

For full control, set `RUST_LOG` to override `MISSING_EBOOKS_LOG` with standard `tracing` filter syntax:

```shell
RUST_LOG=missing_ebooks::scanner=trace
```

This scopes trace-level output to the scanner module and leaves the rest of the app at its default.

## Verbosity control

The primary control is `MISSING_EBOOKS_LOG`, a prefixed variable consistent with the other knobs. It takes a level (`error`, `warn`, `info`, `debug`, `trace`). Raising it to `debug` or `trace` is scoped to this crate, so `tokio`, `hyper`, and `axum` internals stay at the `info` baseline rather than surfacing; lowering it to `warn` or `error` applies everywhere, so those dependencies are never left louder than the app. An unknown value falls back to `info` and logs a warning. `RUST_LOG` is still honored when set, giving developers the full env-filter directive grammar as an escape hatch. Resolution order is `RUST_LOG`, then `MISSING_EBOOKS_LOG`, then a default of `info`. Env-first is deliberate: the subscriber initializes before `Config::load` runs so config errors can be logged, and the app is env-first (see ADR-0004).

## Tiers

Three tiers carry the load. `info` (the default) shows one headline line per full scan plus the existing warnings. `debug` adds per-root scan timing and gap counts, cache hits and misses, marker write and delete timing, and per-request and render latency. `trace` adds a line per directory walked. Each timing event is emitted explicitly with an `Instant` at the operation's completion point, carrying an `elapsed_ms` float field beside the result counts, rather than through tracing spans, so the duration and outcome read on one line and the `spawn_blocking` boundary stays simple.

## Alternatives rejected

A compile-time cargo feature for the expensive trace tier is unnecessary: a disabled `tracing` callsite costs one atomic load and never evaluates its field expressions, so runtime level-gating already keeps the cost off the default path. A metrics or histogram layer is the wrong shape for the goal, which is to read how long a specific scan took on a specific library, not to aggregate percentiles over time.
