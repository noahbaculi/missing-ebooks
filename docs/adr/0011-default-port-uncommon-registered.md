# Default port is 13379, an uncommon high port

Date: 2026-06-06.

## Context

This replaces 8080, which is the single highest-collision choice a web app can make. IANA registers 8080 as `http-alt`, "HTTP Alternate (see port 80)", and it is the default unprivileged web port for Tomcat, Spring Boot, Jenkins, and most dev tooling. The well-known 8081/8082/8083 fallback ladder exists precisely because 8080 is so often already taken.

RFC 6335 (BCP 165) splits the port space into well-known (0-1023), registered (1024-49151), and dynamic/ephemeral (49152-65535). A fixed listen port is a service identifier, so it belongs in the registered range and must stay out of the dynamic range, which the OS hands out for ephemeral outbound connections. Binding a number in the registered range needs no IANA registration, which is why none of the comparable apps register theirs.

## Decision

The server defaults to port 13379, exposes it as a config field with the `MISSING_EBOOKS_PORT` env override, and shows that same number throughout the README so the documented examples match what a real deployment listens on. The runnable `explore` harness prefers this same default so its URL matches a real deployment, and falls back to an OS-assigned port only when 13379 is already taken (for instance when a real instance is running). An explicit `--port` is bound exactly.

The self-hosted media ecosystem this tool sits alongside picks memorable, uncommon registered-range ports rather than 8080: Audiobookshelf 13378, Sonarr 8989, Radarr 7878, Prowlarr 9696, Komga 25600. 13379 is one above Audiobookshelf's 13378, since this tool finds the ebook gaps in the same audiobook library, and the 13xxx band holds no other common service to clash with.

## Consequences

Under Docker the choice matters less than it first looks, but not zero. In the default bridge network each container has its own port space, so the in-container port never conflicts on its own. A host conflict only shows up at publish time and is resolved by mapping a different host port (`-p 9090:13379`) without touching the app config. Host networking mode is the exception: `-p` is ignored there, the configured port is the host port, and a low-collision default is the only thing between the user and a clash. That mode, the compose stacks that run several of the apps above on one host, and matching ecosystem convention are why an uncommon default earns its keep even though bridge mapping covers most deployments. The bind address is a separate knob with its own default (see ADR-0003): the Docker image binds `0.0.0.0` so a published port reaches the process.

Sources: the IANA Service Name and Transport Protocol Port Number Registry, [RFC 6335](https://www.rfc-editor.org/rfc/rfc6335.html), the Docker networking docs ([port publishing](https://docs.docker.com/engine/network/port-publishing/) and [host driver](https://docs.docker.com/engine/network/drivers/host/)), and the projects' own docs (Audiobookshelf, Komga, the Servarr wiki).
