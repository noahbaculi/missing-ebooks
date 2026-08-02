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
