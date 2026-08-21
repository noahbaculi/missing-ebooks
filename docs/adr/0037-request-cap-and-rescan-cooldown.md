# Requests cap at 16 in flight and rescans cool down for 5 seconds

Date: 2026-07-30.

## Context

`router()` mounted no limiting layer, so nothing bounded concurrent page renders (each buffers the whole library into one String, ADR-0032), and an unauthenticated `POST /rescan` loop discarded the mtime index and forced an uncapped cold walk per request, a durable denial of service on the network mounts this tool targets.

## Decision

A `tower::limit::GlobalConcurrencyLimitLayer` caps the router at 16 concurrently served requests through one shared semaphore. Excess requests queue rather than erroring. The store records the instant of the last honored rescan. A rescan landing within 5 seconds skips the index clear (the expensive part the cooldown defends against) but still re-walks with the warm per-root index in place, via `build_coalesced`, joining an in-flight walk if one is already running rather than starting a redundant one. A double-click or tight request loop still coalesces onto whatever walk is already in progress, but a rescan landing after that walk finishes gets an honest warm re-walk of its own rather than a cached, possibly stale, result: clicking rescan, fixing a file by hand, and clicking rescan again inside the window still picks up the fix.

A request timeout layer was considered and rejected. A timeout is the only guardrail that can kill a legitimate cold-scan page load on a slow network mount, and the cap plus the cooldown cover the audited abuse cases. A generous timeout can be added later if a hang is ever observed.

## Consequences

A rescan loop no longer keeps the server permanently cold-walking, and slow readers cannot pile up unbounded page buffers. The 17th concurrent request waits instead of failing, the honest behavior for a self-hosted tool. Buffered rendering itself stays as ADR-0032 chose it.
