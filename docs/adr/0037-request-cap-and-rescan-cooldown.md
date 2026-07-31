# Requests cap at 16 in flight and rescans cool down for 5 seconds

Date: 2026-07-30.

## Context

`router()` mounted no limiting layer, so nothing bounded concurrent page renders (each buffers the whole library into one String, ADR-0032), and an unauthenticated `POST /rescan` loop discarded the mtime index and forced an uncapped cold walk per request, a durable denial of service on the network mounts this tool targets.

## Decision

A `tower::limit::GlobalConcurrencyLimitLayer` caps the router at 16 concurrently served requests through one shared semaphore. Excess requests queue rather than erroring. The store records the instant of the last honored rescan; a rescan landing within 5 seconds skips the index clear and joins the in-flight or fresh build via `build_coalesced`, returning normally. Silent coalescing, no error UI: a double-click or a request loop costs one walk.

A request timeout layer was considered and rejected. A timeout is the only guardrail that can kill a legitimate cold-scan page load on a slow network mount, and the cap plus the cooldown cover the audited abuse cases. A generous timeout can be added later if a hang is ever observed.

## Consequences

A rescan loop no longer keeps the server permanently cold-walking, and slow readers cannot pile up unbounded page buffers. The 17th concurrent request waits instead of failing, the honest behavior for a self-hosted tool. Buffered rendering itself stays as ADR-0032 chose it.
