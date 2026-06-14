# Benchmarks

`scan_bench` times the real scanner against the configured library roots, cold and warm, and writes a JSON report. The reports here compare a local mergerfs root on jane-core against the same library reached over an SMB (CIFS) mount on jane-2. The harness is `examples/scan_bench.rs`; the analysis is [ADR-0019](../docs/adr/0019-scan-walk-parallel-sized-by-concurrency.md).

## The reports

| File | Host | Backend | Schema | Notes |
| ---- | ---- | ------- | ------ | ----- |
| `scan-bench-local-jane-core-1781322846.json` | jane-core | fuse.mergerfs | 1 | one concurrency, pre-sweep |
| `scan-bench-local-jane-core-1781329903.json` | jane-core | fuse.mergerfs | 2 | concurrency sweep 1,4,8,16,32 |
| `scan-bench-smb-jane-2-1781322985.json` | jane-2 | cifs | 1 | one concurrency, pre-sweep |
| `scan-bench-smb-jane-2-1781329970.json` | jane-2 | cifs | 2 | concurrency sweep 1,4,8,16,32 |
| `scan-bench-smb_single_channel-jane-2-1781418650.json` | jane-2 | cifs | 2 | caching on, single channel, sweep 1,4,8,16,32 |
| `scan-bench-smb_multi_channel-jane-2-1781419177.json` | jane-2 | cifs | 2 | caching on, multichannel, sweep 1,4,8,16,32 |

Schema 2 adds the per-concurrency `levels` array; schema 1 reports a single concurrency inline. The trailing number is the unix time, so the files sort by run.

## What they show

The schema-2 sweeps establish two things about an SMB library. Concurrency is a large win locally, the full warm walk falling about sevenfold from serial to 16 threads with the elbow at 8, and close to inert over SMB, gaining only about 1.3x across the whole sweep, because the server answers one connection's requests in order. And warm runs even with cold over SMB, because the CIFS client's attribute cache ages out within a second, faster than a multi-second walk finishes. At the default 16 threads the same ~900-folder library is about 17 ms warm locally against about 1.8 s over SMB. ADR-0019 carries the full reasoning.

## Regenerating

Build with `--release` so the timings reflect the shipped binary. `--drop-caches` flushes the page cache before each cold run; the harness escalates that one step with its own `sudo`, so cache your credentials with `sudo -v` first and run cargo as the normal user. Point it at the real mounts with `--root PATH` (repeatable), `--config config.toml`, or `MISSING_EBOOKS_LIBRARY_ROOTS`.

On the host that holds the library:

```shell
sudo -v
cargo run --release --example scan_bench -- --root /mnt/pool/Entertainment/Audiobooks --label local --drop-caches --concurrency 1,4,8,16,32
```

On a client that mounts the library over SMB:

```shell
sudo -v
cargo run --release --example scan_bench -- --root /mnt/jane-nas/Entertainment/Audiobooks --label smb --drop-caches --concurrency 1,4,8,16,32
```

## SMB deployment experiments

A protocol for the levers that might cut SMB scan time, to run before any of them reach the README "Network shares" section. Run each on jane-core (the storage host and Samba server) and jane-2 (the CIFS client), measure with `scan_bench`, and keep only the levers that move the numbers. The config changes live in the server-configs repo: jane-2 fstab and jane-core `smb.conf`.

Baseline, measured 2026-06-13: full walk, warm, 16 threads is about 1.8 s over SMB and about 17 ms local; SMB warm equals cold; SMB concurrency tops out near 1.3x.

### 0. Control

Re-run the sweep on both hosts unchanged, so the rest read against a current baseline on the same hardware. Use the two commands under Regenerating. Confirms the local-versus-SMB gap and the flat SMB curve still hold.

### 1. Client caching

On jane-2, add `actimeo=30` to the CIFS line in fstab and remount; then try `cache=loose` separately. Re-run the SMB sweep. Confirmed if warm drops well below the ~1.8 s baseline while cold is unchanged, meaning the second walk within the window now hits the client cache. Trade-off: a newly written ebook or marker appears up to `actimeo` seconds later, which the rescan button covers.

Result (2026-06-14, jane-2): rejected, reverted to the CIFS defaults. Against the `cache=strict,actimeo=1` baseline, `cache=loose,actimeo=30` cut the serial walk by about 14% and lowered the curve a little, but warm still ran even with cold (1653 against 1714 ms at 16 threads). The walk waits on per-directory readdir round trips, which the attribute cache does not serve, not the per-file stats `actimeo` caches, so the gain is serial-only, not worth an up-to-30 s staleness window for a gap finder. Reports: `scan-bench-smb-jane-2-1781329970.json` (defaults baseline) against `scan-bench-smb_single_channel-jane-2-1781418650.json` (caching on).

### 2. SMB3 multichannel

Needs both ends. On jane-core, set `server multi channel support = yes` in `smb.conf` and restart Samba. On jane-2, the CIFS client will not open extra channels unless told, so add `multichannel,max_channels=4` to the mount options and remount. Confirm channels actually opened before trusting the timing:

```shell
cat /proc/fs/cifs/DebugData   # expect more than one channel listed
```

Re-run the SMB sweep. Confirmed if the SMB curve starts scaling with concurrency the way the local curve does, beating serial by more than the current 1.3x. This is the only lever that could break the single-connection ceiling. Samba multichannel is still flagged experimental, so it is the most likely to come back negative.

Result (2026-06-14, jane-2): shelved (not disproven) and reverted on both ends. The full warm walk gained 1.27x serial-to-best on single channel against 1.26x on multichannel, within noise at every matched concurrency, so the curve never started scaling. DebugData (`cifs-debugdata-jane-2-redacted.txt`) shows why: the connection allocated exactly one channel. The server had multichannel on and advertised four interfaces, but the one the mount uses reports `Capabilities: None` (no RSS, so no second channel to the same IP), and its two 10 Gbps interfaces are docker-internal addresses the LAN client cannot reach. Engaged multichannel was therefore never tested. This is inapplicable on this host rather than a lever that failed to help. A revisit needs an RSS-capable interface or a second reachable NIC, with Samba's advertised `interfaces` restricted so it stops offering docker IPs, and more than one allocated channel confirmed in DebugData before trusting the timing. Reports: `scan-bench-smb_single_channel-jane-2-1781418650.json` against `scan-bench-smb_multi_channel-jane-2-1781419177.json`.

### Recording results

For a lever that pays off, add a note to the README "Network shares" section and commit the matching server-configs change. Record a lever that does not move the numbers here as tested and rejected, so it is not retried.
