# ADR-0038: The container runs as an ordinary Docker user

Date: 2026-08-04.

## Context

An earlier iteration shipped the image with a 27-line `docker/entrypoint.sh` that dropped root to `PUID`/`PGID` via `su-exec` and sniffed for a mounted `/config/config.toml`. Its stated reason was that the PUID convention "sidesteps the permission friction of a UID baked into the image".

That reason does not survive contact with the alternative. A baked-in UID was never the choice on the table. `--user 1000:1000` is set at run time and bakes in nothing, so it sidesteps the identical friction with no code, no `su-exec` package, no test script, and no CI job.

The prior art agrees, including the project this one already mirrors. Audiobookshelf, whose extension lists ADR-0006 follows, was asked for PUID/PGID support in [issue #3527](https://github.com/advplyr/audiobookshelf/issues/3527) and declined: a contributor answered "you should use the `user` directive because this is supported by docker itself instead of requiring individual containers to correctly use `PUID` or `GUID`", and the owner added "I don't see the benefit of adding environment variables when there is `user` built-in." Audiobookshelf, navidrome, and gickup all run the binary straight from `ENTRYPOINT`.

LinuxServer.io, where the convention comes from, does not defend it on design grounds either. Their [PUID documentation](https://docs.linuxserver.io/general/understanding-puid-and-pgid/) says: "We are aware that recent versions of the Docker engine have introduced the `--user` flag. Our images are not yet compatible with this, so we recommend continuing usage of PUID and PGID." That is a fact about s6-overlay base images needing root at startup, not an argument that a single static Rust binary should grow a shim. This one does nothing at startup that requires root and binds a port above 1024.

No git tag exists, `docker-publish.yml` fires only on `v*`, so nothing was ever published to GHCR and nobody is running the image. This is pre-release cleanup, not a migration.

## Decision

The runtime stage runs `ENTRYPOINT ["/usr/local/bin/missing-ebooks"]` as `USER 1000:1000`, numeric so no account has to exist in the image. Compose `user:` and `docker run --user` override it exactly as `PUID` did. The shim, its `su-exec` package, its shell test, and its CI job are deleted.

The `/config/config.toml` convention survives as `ENV MISSING_EBOOKS_CONFIG=/config/config.toml`, which is safe because the binary now treats an env-provided path as a hint: `resolve_config_path` in `src/main.rs` drops it when no file exists there, while an explicit `--config` still errors on a bad path. Knowledge of the `/config` convention lives in the Dockerfile, not in the app.

Defaulting to non-root is a deliberate deviation from the comparables, which default to root and leave `user:` to the operator. Three reasons it is right here.

First, this app writes into the user's library rather than into app-owned storage. Audiobookshelf writes to dedicated `./metadata` and `./config` mounts, which an operator treats as opaque, so root-owned files there stay contained. This app writes `.no_ebook` and `.ebook_elsewhere` directly into audiobook folders, interleaved with media across a hand-curated tree. As root, every mark scatters a root-owned dotfile the operator cannot delete without sudo, and marking is the whole point of the tool rather than an occasional side effect.

Second, the write endpoint is unauthenticated, and root widens its blast radius. ADR-0008 calls its canonicalize-and-prefix-check guard "the second layer behind ADR-0003". As uid 1000 a bypass writes a file as uid 1000; as root it writes anywhere in the container as root. Running at maximum privilege removes the layer beneath a control designed as defense in depth.

Third, the repo already made this call in writing. ADR-0003 deviates from the same set of projects on binding, reasoning that "every one of those ships built-in authentication, which is what makes a broad bind safe. This tool has no auth and an unauthenticated write endpoint (it creates marker files on disk), so it follows the security-first camp instead." The sentence holds verbatim with "runs as root" swapped in for "binds all interfaces".

It is also parity rather than a new stance: `PUID` defaulted to 1000 already, so root would be a regression adopted during a cleanup.

## Consequences

The image starts as a non-root user, which the shim never achieved, and roughly 105 lines come out across the Dockerfile, the shim, its test, and its CI job. One package and two build layers leave the runtime image, one more leaves the demo stage, and a job comes off every push. Added back: a four-line function with three unit tests, and two Dockerfile lines.

The cost lands on an operator whose library is owned by another uid and who sets no `user:`. They get a failure where root would have worked silently. It is partial and late rather than immediate: a world-readable library still scans and renders fine, and the error appears on the first mark click. It surfaces properly rather than silently (`src/state.rs` covers this with `write_marker_reports_a_permission_failure_not_a_missing_target`), but no startup probe warns at boot. A wrong `PUID` produced the same wrinkle, so this does not worsen it. Lowering it would take a startup writability check per configured root, logging a warning that names the uid mismatch. That is a follow-up, deliberately out of scope here.

Known limitation, recorded rather than solved: Docker creates named volumes root-owned, so a non-root container cannot write to a fresh one. Every mount in the shipped compose files is a bind mount onto a host-owned path, and the app writes nothing outside the library, so nothing here is affected. It would bite if state ever moved into a named volume.

The three unit tests on `resolve_config_path` replace the 67-line shell test. The publish smoke curl covers the entrypoint change, since a broken `ENTRYPOINT` means no server answers.

## Related

Cites ADR-0003, ADR-0006, ADR-0008.
