# Network shares

Pointing a library root at an SMB or NFS mount isn't encouraged, as the scan is far slower than on local disk and there is a firm limit on how much it can be sped up. The walk reads every folder, and each folder costs a handful of round trips to open, list, and close it, with none per file, so scan time scales with the number of folders rather than the number of files.

Knob definitions live in the annotated `config.toml` in the README. This page layers SMB/NFS-specific tuning on top of them.

`scan_concurrency`: on local storage, raising it is roughly a sevenfold win at the default. In my test lab, this unfortunately had very little effect over SMB: the server answers one connection's requests in order, so the extra readers fold back onto one and the walk gains only about a third (see [ADR-0019](adr/0019-scan-walk-parallel-sized-by-concurrency.md)). Set the value by the speed of your NAS, not your CPU count. The readers spend almost all their time waiting on the network, so they cost little CPU even well above the core count, and raising a container's `--cpus` does not help.

`ttl_seconds`: raise it on a slow mount and treat the `Rescan` UI button as the deliberate refresh. Cached views matter more over SMB than locally.

`poll_interval_seconds`: if your SMB link is too slow for warm rescanning to work effectively, set it to `0` to disable the poll so that clients refresh only via the `Rescan` UI button that triggers a cold scan.

Staleness detection keys off directory mtime. On filesystems with coarse or unreliable mtime (some NFS and FAT mounts), a change made inside the same mtime tick can be missed until the next cold rescan. Use the `Rescan` UI button or a shorter TTL if your mount has this problem.
