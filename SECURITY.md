# Security policy

## Reporting a vulnerability

Report privately via [GitHub Security Advisories](https://github.com/noahbaculi/missing-ebooks/security/advisories/new). If that channel is not workable, email the maintainer through the contact address on the [GitHub profile](https://github.com/noahbaculi). Please do not open a public issue for suspected vulnerabilities.

Expect an acknowledgement within a few days and a fix or mitigation plan once the report is triaged.

## Supported versions

Once v1.0.0 is out, security fixes land on the latest minor line. Older minors do not receive backports.

| Version      | Supported |
| ------------ | --------- |
| Latest minor | Yes       |
| Older minors | No        |

## Deployment posture

`missing-ebooks` has no authentication, no authorization, and no session concept. It is meant to run on loopback and sit behind a reverse proxy that enforces auth before any non-local access. See the [README warning](README.md#missing-ebooks) and [ADR-0003](docs/adr/0003-default-bind-loopback.md) for the default bind rationale.

The shipped [`docker-compose.yml`](docker-compose.yml) and [`docker-compose.advanced.yml`](docker-compose.advanced.yml) run the container with `read_only: true`, `cap_drop: [ALL]`, `security_opt: ["no-new-privileges:true"]`, and `tmpfs: /tmp`. The app writes only to the mounted library, so read-only rootfs costs nothing, and the other flags remove capabilities and privilege-escalation paths a compromised process would otherwise inherit. Keep them set.

Binding to a non-loopback address (`0.0.0.0`, a LAN IP, a tailnet IP) is refused at startup unless `MISSING_EBOOKS_ALLOW_PUBLIC_BIND=1` is set. Setting it acknowledges the trust boundary described below: the server still ships with no auth, and any non-loopback bind belongs behind a reverse proxy that enforces one.

## Threat model on a non-loopback bind

Anything on the same network as an unauthenticated `0.0.0.0` instance can:

- Enumerate the full folder tree under every configured library root, including filenames and any path components that carry personal information.
- Plant `.no_ebook` or `.ebook_elsewhere` markers under any library root via `POST /mark` and `POST /unmark`. Marker writes are guarded against `..` traversal ([ADR-0008](docs/adr/0008-marker-write-guarded-against-traversal.md)); known caveats around symlink TOCTOU, cross-mount, and Unicode are tracked in [issue 11](.scratch/v1-readiness/issues/11-marker-guard-toctou-mount-unicode.md).
- Force repeated cold scans through `POST /rescan`, driving disk and CPU. The 16 in-flight request cap and rescan cooldown ([ADR-0037](docs/adr/0037-request-cap-and-rescan-cooldown.md)) limit the blast radius but do not stop a determined peer.

## What is not defended against

- No CSRF token on `POST /mark`, `POST /unmark`, `POST /rescan`. A browser that can reach the server can be tricked into issuing writes via cross-origin form submits.
- No per-IP rate limit beyond the global in-flight cap.
- No transport encryption. Terminate TLS at the reverse proxy.

Any deployment that exposes the raw server past loopback is trusted-network only.

## Release supply chain

The publish workflow ([`.github/workflows/docker-publish.yml`](.github/workflows/docker-publish.yml)) re-runs `cargo deny check all` on the tagged commit before the image builds, so a red advisory or license check blocks publish even if the tag was cut against a red `main`.
