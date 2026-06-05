# Default bind address is 127.0.0.1, configurable

The server binds `127.0.0.1` by default, exposes the bind address as a config
field with an env override (so it can be set to `0.0.0.0` or a specific
interface), and logs a warning at startup when bound to a non-loopback address.

This deviates from the prevailing homelab/media-app convention of binding all
interfaces (`0.0.0.0`/`::`) out of the box (Gitea, Paperless-ngx, Immich,
Audiobookshelf). Every one of those ships built-in authentication, which is what
makes a broad bind safe. This tool has no auth and an unauthenticated write
endpoint (it creates marker files on disk), so it follows the security-first camp
instead. Syncthing's GUI is the direct analog: single-user, no-auth, deliberately
bound to loopback as a "reasonably safe default"
([issue #3357](https://github.com/syncthing/syncthing/issues/3357),
[guilisten.html](https://docs.syncthing.net/users/guilisten.html)); Miniflux
defaults to loopback for the same reason. The *arr stack shows the alternative's
cost: it made auth mandatory rather than restrict the bind, and the "disabled for
local addresses" relaxation still produced an auth-bypass CVE
(GHSA-h5qx-5hjf-7c9r). Loopback also composes with the intended deployment, since
Cloudflare Tunnel and `tailscale serve` both reach a localhost origin, so the
secure default is also the working default. The Docker image sets `0.0.0.0`
explicitly (the native-vs-container divergence Syncthing documents), with
exposure controlled at the port-publish layer.
