# Benchmarks experiment log, 2026-06

One developer's homelab notes. Host names (`jane-core`, `jane-2`, `jane-nas`) refer to the author's personal infrastructure and are not meant to be reproducible elsewhere.

## Protocol

A protocol for the levers that might cut SMB scan time and the checks that bound it, to run before any of them reach the README "Network shares" section. Run each on jane-core (the storage host and Samba server) and jane-2 (the CIFS client), measure with `scan_bench`, and keep only the levers that move the numbers. The config changes live in the server-configs repo: jane-2 fstab and jane-core `smb.conf`.

Baseline, measured 2026-06-13: full walk, warm, 16 threads is about 1.8 s over SMB and about 17 ms local; SMB warm equals cold; SMB concurrency tops out near 1.3x.

## Reports referenced

| File | Host | Backend | Schema | Notes |
| ---- | ---- | ------- | ------ | ----- |
| `scan-bench-local-jane-core-1781322846.json` | jane-core | fuse.mergerfs | 1 | one concurrency, pre-sweep |
| `scan-bench-local-jane-core-1781329903.json` | jane-core | fuse.mergerfs | 2 | concurrency sweep 1,4,8,16,32 |
| `scan-bench-smb-jane-2-1781322985.json` | jane-2 | cifs | 1 | one concurrency, pre-sweep |
| `scan-bench-smb-jane-2-1781329970.json` | jane-2 | cifs | 2 | concurrency sweep 1,4,8,16,32 |
| `scan-bench-smb_single_channel-jane-2-1781418650.json` | jane-2 | cifs | 2 | caching on, single channel, sweep 1,4,8,16,32 |
| `scan-bench-smb_multi_channel-jane-2-1781419177.json` | jane-2 | cifs | 2 | caching on, multichannel, sweep 1,4,8,16,32 |
| `scan-bench-local-jane-core-1781548391.json` | jane-core | fuse.mergerfs | 2 | control re-run, all + gaps, sweep 1,4,8,16,32 |
| `scan-bench-smb-jane-2-1781548745.json` | jane-2 | cifs | 2 | control re-run, all + gaps, sweep 1,4,8,16,32 |
| `scan-bench-local-jane-core-1781552857.json` | jane-core | fuse.mergerfs | 3 | adds incremental mode, single level at top concurrency |
| `scan-bench-smb-jane-2-1781552965_1.json` | jane-2 | cifs | 3 | adds incremental mode, single level at top concurrency |
| `scan-bench-local-jane-core-1782054501.json` | jane-core | fuse.mergerfs | 4 | adds `tree_build_ms` per level |
| `scan-bench-smb-jane-2-1782054802.json` | jane-2 | cifs | 4 | adds `tree_build_ms` per level |
| `scan-bench-local-jane-core-1782056191_1.json` | jane-core | fuse.mergerfs | 5 | `iteration_counts` canary clean, gate PASS at 14.4x cold |
| `scan-bench-smb-jane-2-1782056306.json` | jane-2 | cifs | 5 | `iteration_counts` canary clean, gate PASS at 9.8x cold |

The `cifs-*` text files in this directory back the rejected `actimeo` and multichannel experiments below.

## What the schema-2 sweeps showed

The schema-2 sweeps establish two things about an SMB library. Concurrency is a large win locally, the full warm walk falling about sevenfold from serial to 16 threads with the elbow at 8, and close to inert over SMB, gaining only about 1.3x across the whole sweep, because the server answers one connection's requests in order. And warm runs even with cold over SMB, because the CIFS client's attribute cache ages out within a second, faster than a multi-second walk finishes. At the default 16 threads the same ~900-folder library is about 17 ms warm locally against about 1.8 s over SMB. ADR-0019 carries the full reasoning.

The reports also time both walk modes, and on this library the gaps-only walk is not meaningfully faster than the full walk. Both visit all 901 directories and see all 8802 entries: coverage-pruning drops nothing because the ebooks sit at the leaves, with no descendants beneath a covered folder to prune. The walks diverge only in the audio tally (1479 against 5556), since the gaps walk stops counting a folder's audio once it is flagged, not in the directories opened. Warm at 16 threads that is about 1825 ms gaps against 1832 ms full over SMB and 16.9 against 17.4 ms local, within noise either way. The per-directory round-trip floor sets the time; pruning earns its keep only on a tree deep enough that covered folders sit above whole subtrees, which a flat and wide library like this one does not have.

## Reproducing on these hosts

On jane-core, the host that holds the library:

```shell
sudo -v
cargo bench --bench scan_bench -- --root /mnt/pool/Entertainment/Audiobooks --label local --drop-caches --concurrency 1,4,8,16,32
```

On jane-2, the client that mounts the library over SMB:

```shell
sudo -v
cargo bench --bench scan_bench -- --root /mnt/jane-nas/Entertainment/Audiobooks --label smb --drop-caches --concurrency 1,4,8,16,32
```

## 0. Control

Re-run the sweep on both hosts unchanged, so the rest read against a current baseline on the same hardware. Use the two commands above. Confirms the local-versus-SMB gap and the flat SMB curve still hold.

## 1. Client caching

On jane-2, add `actimeo=30` to the CIFS line in fstab and remount; then try `cache=loose` separately. Re-run the SMB sweep. Confirmed if warm drops well below the ~1.8 s baseline while cold is unchanged, meaning the second walk within the window now hits the client cache. Trade-off: a newly written ebook or marker appears up to `actimeo` seconds later, which the rescan button covers.

Result (2026-06-14, jane-2): rejected, reverted to the CIFS defaults. Against the `cache=strict,actimeo=1` baseline, `cache=loose,actimeo=30` cut the serial walk by about 14% and lowered the curve a little, but warm still ran even with cold (1653 against 1714 ms at 16 threads). The walk waits on per-directory readdir round trips, which the attribute cache does not serve, not the per-file stats `actimeo` caches, so the gain is serial-only, not worth an up-to-30 s staleness window for a gap finder. Reports: `scan-bench-smb-jane-2-1781329970.json` (defaults baseline) against `scan-bench-smb_single_channel-jane-2-1781418650.json` (caching on).

## 2. SMB3 multichannel

Needs both ends. On jane-core, set `server multi channel support = yes` in `smb.conf` and restart Samba. On jane-2, the CIFS client will not open extra channels unless told, so add `multichannel,max_channels=4` to the mount options and remount. Confirm channels actually opened before trusting the timing:

```shell
cat /proc/fs/cifs/DebugData   # expect more than one channel listed
```

Re-run the SMB sweep. Confirmed if the SMB curve starts scaling with concurrency the way the local curve does, beating serial by more than the current 1.3x. This is the only lever that could break the single-connection ceiling. Samba multichannel is still flagged experimental, so it is the most likely to come back negative.

Result (2026-06-14, jane-2): shelved (not disproven) and reverted on both ends. The full warm walk gained 1.27x serial-to-best on single channel against 1.26x on multichannel, within noise at every matched concurrency, so the curve never started scaling. DebugData (`cifs-debugdata-jane-2-redacted.txt`) shows why: the connection allocated exactly one channel. The server had multichannel on and advertised four interfaces, but the one the mount uses reports `Capabilities: None` (no RSS, so no second channel to the same IP), and its two 10 Gbps interfaces are docker-internal addresses the LAN client cannot reach. Engaged multichannel was therefore never tested. This is inapplicable on this host rather than a lever that failed to help. A revisit needs an RSS-capable interface or a second reachable NIC, with Samba's advertised `interfaces` restricted so it stops offering docker IPs, and more than one allocated channel confirmed in DebugData before trusting the timing. Reports: `scan-bench-smb_single_channel-jane-2-1781418650.json` against `scan-bench-smb_multi_channel-jane-2-1781419177.json`.

## 3. Per-entry round trips in the scanner walk

Not a deployment lever but a check on our own code, run on the CIFS client. The walk calls `entry.file_type()` on every entry, which over CIFS is free when the directory query populated `d_type` and one stat round trip per entry when it returns `DT_UNKNOWN`; the `reparse=nfs,mapposix` mount made the unknown case plausible. Trace a single serial gaps walk under `strace -c -e trace=getdents64,newfstatat,statx,lstat`, diff `/proc/fs/cifs/Stats` around the same walk, and check whether the stat round trips track `dirs_visited` or `entries_seen`. The full method lives in git history, in the experiment spec this result replaced.

Result (2026-06-14, jane-2): confirmed round-trip-minimal, no scanner change warranted. The stat family was three `statx` calls total against 17,604 entry-visits, so `file_type()` is served from `d_type` with no syscall, and `newfstatat` and `lstat` never fired. On the wire `QueryInfos` tracked directories rather than entries (0.06 per entry), while `QueryDirectories`, `Creates`, and `Closes` all tracked directory count. The floor is about seven SMB2 ops per directory (open, two `QUERY_DIRECTORY`, close, plus a little metadata), served in order by one `smbd`, the protocol handshake the kernel CIFS client issues rather than anything the scanner adds. Reports: `cifs-roundtrip-strace-jane-2-1781471709.txt` (Method A, syscalls) against `cifs-roundtrip-stats-jane-2-1781471709.txt` (Method B, wire).

## 4. Incremental rescan (stat vs list)

A check on the in-memory mtime index that warm scans reuse (the dir index, ADR-0020). At the time this experiment ran, the warm path was gated by an `incremental_scan` config knob; a later change removed the knob and made warm scans the default for every scan path. The result here is what justified that change. On jane-2, against the SMB mount, run the incremental bench mode after the unchanged-tree assumption holds (no writes to the share between the walks). `--drop-caches` is what makes this honest over SMB: a reuse walk stats every directory in well under a second, shorter than the attribute cache window (actimeo=1 on this mount), so back-to-back warm walks hit the cached attrs and the warm median understates the real per-directory round trip. The cold phase drops the client cache before each walk, so every stat is a real round trip, the way a rescan minutes after the last one will be.

```shell
sudo -v
cargo bench --bench scan_bench -- --root /mnt/jane-nas/Entertainment/Audiobooks --iterations 5 --drop-caches --label smb
```

A bare run uses the default `every` mode, which times the full-listing walk, the gaps walk, and the reuse walk in one pass and saves them to a single JSON report, so the go/no-go ratio is computable from one file with no hand-transcription. The `warm` mode (named `incremental` in the historical reports linked above) is a single-level entry: it runs once at the top concurrency rather than sweeping, since concurrency is inert over SMB for the reuse walk.

Confirmed if the warm level (`incremental` in the historical reports) reports `dirs_reused` equal to `dirs_visited` and `entries_seen=0`, and its cold median sits well below the `full` mode's cold median (the ~1.8 s full listing walk in the same file), matching the projected drop from about six or seven SMB2 ops per directory to about two or three. The warm median is the cache-hot best case, not the figure to judge against. The harness prints a one-line `gate: PASS/FAIL` per root that wraps both checks; treat it as the headline and inspect the JSON for the supporting numbers. The unchanged-tree assumption is also auditable in the report: each warm phase records `iteration_counts`, one row per iteration, and an iteration whose `dirs_reused` or `entries_seen` drifts mid-run is the canary that something external (backup, indexer, beets) wrote to the tree between walks. Then confirm directory mtime actually moves over the mount: add an ebook into one folder on the server, re-run, and check that exactly that folder re-lists (`dirs_reused` falls by one and `entries_seen` rises); diffing the two saved reports shows it. If the cold reuse walk is not meaningfully cheaper, or mtime does not move on an add, shelve the warm path the way multichannel was shelved and leave it documented but unrecommended.

Record the result here and, if it pays off, add a note to the README "Network shares" section.

Result (2026-06-21, jane-2 and jane-core): primary gate passed, warm scans become the default scan path; the mtime-moves-on-an-add half of the gate is the open follow-up. Over SMB the cold incremental median was 260 ms against 2539 ms for the serial full walk (9.8x) and 1937 ms for the full walk at the best concurrency (7.5x), with reuse 901/901 dirs and entries_seen=0. Cold is the honest figure because the warm reuse walk (1.4 ms median) finishes inside the actimeo=1 attribute cache window. The schema-5 `iteration_counts` rows held steady at `dirs_reused=901, entries_seen=0` across all five iterations of both cold and warm phases on both hosts, so the unchanged-tree assumption held while the timer ran. The jane-core local control (same library, fuse.mergerfs) hit the gate with a 14.4x ratio (23 ms incremental cold against 338 ms serial full), confirming the win is not specific to network latency. A schema-4 pair recorded ~25 minutes earlier produced the same incremental medians within noise (270 ms SMB cold, 24 ms local cold). Reports: `scan-bench-smb-jane-2-1782056306.json` and `scan-bench-local-jane-core-1782056191_1.json` (schema 5, gate PASS), with schema-4 noise-bounds `scan-bench-smb-jane-2-1782054802.json` and `scan-bench-local-jane-core-1782054501.json`.

## Recording results

For a lever that pays off, add a note to the README "Network shares" section and commit the matching server-configs change. Record a lever that does not move the numbers here as tested and rejected, so it is not retried.
