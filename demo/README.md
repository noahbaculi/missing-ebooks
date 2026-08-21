# missing-ebooks demo site

A public, ephemeral demo of missing-ebooks. One server process serves every visitor. Each visitor gets a private view seeded with sample data, and their changes are kept in memory and reset after a few idle minutes. The stack is the demo server fronted by a Cloudflare Tunnel. No inbound ports are opened on the host.

## What runs where

- The host is any always-on Linux box. The reference host is an Oracle Cloud always-free Ampere VM (arm64), which builds the arm64 image natively.
- Two containers run: `demo` (the single server process) and `cloudflared` (the tunnel client).
- Cloudflare terminates TLS for `demo-missing-ebooks.noahbaculi.com` and routes it to the demo server over the tunnel. A single-label subdomain keeps it under free Universal SSL.

## How isolation works

One process serves all visitors. The seeded library is scanned once at startup into shared, read-only base views. A session cookie pins each visitor to an in-memory set of marks. On every request the server clones the base view and replays that session's marks on top, so each visitor sees only their own changes. Marks never touch disk, the data is synthetic, and an idle reaper recycles abandoned sessions. A global session cap bounds memory. At the cap a new visitor gets a soft 503 page. Request volume is throttled at the Cloudflare edge.

## One-time host setup

1. Provision the VM and install Docker with the compose plugin.
2. Clone this repo on the host.

## One-time tunnel setup

1. In the Cloudflare Zero Trust dashboard, go to Networks > Tunnels and create a tunnel (named, for example, `missing-ebooks-demo`).
2. On the connector install screen, choose Docker and copy the token value that follows `--token`.
3. Add a public hostname to the tunnel:
   - Subdomain: `demo-missing-ebooks`
   - Domain: `noahbaculi.com`
   - Service: `HTTP`, URL `demo:8080`
4. Copy `demo/.env.example` to `demo/.env` and paste the token into `TUNNEL_TOKEN`.

## Go live

From the repo root on the host:

```bash
docker compose -f demo/docker-compose.yml --env-file demo/.env up -d --build
```

Then open https://demo-missing-ebooks.noahbaculi.com. The first request mints a session and drops you into the live UI with the demo banner.

## Required edge protection

The demo ships no in-app rate limiter, so all request throttling lives at the Cloudflare edge. Set this up before going live, in the Cloudflare dashboard for `noahbaculi.com`:

- Add a rate-limiting rule scoped to the demo hostname. It is the only thing bounding how fast a single client can mint sessions toward the global cap.

## Edge protections (recommended)

Also in the Cloudflare dashboard:

- Enable Bot Fight Mode.
- Leave the managed WAF ruleset on.

## Operations

```shell
docker compose -f demo/docker-compose.yml logs -f demo     # follow the logs
docker compose -f demo/docker-compose.yml up -d --build    # update after a code change
docker compose -f demo/docker-compose.yml down             # stop and remove the containers (sessions are in-memory)
```

Tune the demo by editing the `DEMO_*` environment values in `demo/docker-compose.yml` (scenario, idle window, session cap, and the cookie name), then re-run the up command.

| Variable | Effect | Default |
| --- | --- | --- |
| `DEMO_BIND` | IP:port to bind | `127.0.0.1:8080` |
| `DEMO_SCENARIO` | Seeded scenario name | `mixed-forest` |
| `DEMO_MAX_SESSIONS` | Hard cap on concurrent sessions | `300` |
| `DEMO_IDLE_SECS` | Session idle window before the reaper drops it | `1200` |
| `DEMO_COOKIE_NAME` | Session cookie name | `me_demo_sid` |

## Notes

- The app has no authentication. That is acceptable here because the data is synthetic and the only per-session write is an in-memory mark that resets when the session is idle. No marker file is written on the request path.
- A container restart drops every in-memory session. There is no on-disk state to clean up.
