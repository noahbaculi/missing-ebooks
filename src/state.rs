//! Application state: the TTL-memoized scan cache behind a mutation lock, plus
//! the immutable `Arc<Config>`. Built in a later increment; see the design spec
//! and docs/adr/0002-v1-runtime-write-model.md.
