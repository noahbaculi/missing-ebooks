# missing-ebooks demo site

A public, ephemeral demo of missing-ebooks. Every visitor gets a private,
throwaway sandbox seeded with sample data; it resets after a few idle minutes.
The stack is a session-router in front of the `explore` harness, fronted by a
Cloudflare Tunnel. No inbound ports are opened on the host.

## What runs where

- **Host:** any always-on Linux box. The reference host is an Oracle Cloud
  always-free Ampere VM (arm64), which builds the arm64 image natively.
- **Containers:** `router` (spawns one `explore` sandbox per visitor and
  reverse-proxies to it) and `cloudflared` (the tunnel client).
- **Cloudflare:** terminates TLS for `demo-missing-ebooks.noahbaculi.com` and
  routes it to the router over the tunnel. A single-label subdomain keeps it
  under free Universal SSL.

## One-time host setup

1. Provision the VM and install Docker with the compose plugin.
2. Clone this repo on the host.

## One-time tunnel setup

1. In the Cloudflare Zero Trust dashboard, go to Networks > Tunnels and create a
   tunnel (named, for example, `missing-ebooks-demo`).
2. On the connector install screen, choose Docker and copy the token value that
   follows `--token`.
3. Add a public hostname to the tunnel:
   - Subdomain: `demo-missing-ebooks`
   - Domain: `noahbaculi.com`
   - Service: `HTTP`, URL `router:8080`
4. Copy `demo/.env.example` to `demo/.env` and paste the token into
   `TUNNEL_TOKEN`.

## Go live

From the repo root on the host:

```bash
docker compose -f demo/docker-compose.yml --env-file demo/.env up -d --build
```

Then open https://demo-missing-ebooks.noahbaculi.com. The first request seeds a
sandbox and drops you into the live UI with the demo banner.

## Edge protections (recommended)

In the Cloudflare dashboard for `noahbaculi.com`:

- Add a rate-limiting rule scoped to the demo hostname.
- Enable Bot Fight Mode.
- Leave the managed WAF ruleset on.

## Operations

- **Logs:** `docker compose -f demo/docker-compose.yml logs -f router`
- **Update after a code change:**
  `docker compose -f demo/docker-compose.yml up -d --build`
- **Tuning:** edit the `ROUTER_*` environment values in
  `demo/docker-compose.yml` (idle window, caps, port range, scenario) and
  re-run the up command.
- **Reset everything:** `docker compose -f demo/docker-compose.yml down` removes
  the containers; sandboxes are ephemeral, so nothing else needs cleanup.

## Notes

- The app has no authentication. That is acceptable here because each sandbox is
  isolated, seeded with synthetic data, and thrown away. Marker writes land only
  in the visitor's own temp directory and cannot escape it.
- A container restart reaps all sandbox processes; the router also clears
  leftover `/tmp/explore-*` directories on startup.
