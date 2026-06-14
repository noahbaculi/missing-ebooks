# SMB round-trip count: per directory or per entry?

An experiment for an agent to run on a CIFS client host, to settle the one SMB question the existing reports leave open. Read-only: it times and traces the real scanner against the live SMB mount and writes nothing to the library.

ADR-0019 established that the SMB walk folds onto roughly one server worker and gains only about 1.3x from concurrency. It did not establish the round-trip count per directory. `scanner::read_dir_gaps` and `read_dir_all` call `entry.file_type()` on every entry. Over CIFS that is free when the directory query already populated `d_type`, and one stat round trip per entry when `d_type` comes back `DT_UNKNOWN`. The jane-2 mount carries `reparse=nfs` and `mapposix`, where unknown `d_type` is plausible, at least for reparse-point entries. The last SMB run saw 8802 entries against 901 directories, so the worst case is close to a round trip per entry instead of per directory, roughly a 10x amplification the scanner would be issuing itself. This decides whether the floor is the protocol or our own walk, before the README tells a no-choice SMB user the slowness is inherent.

## Where to run

The CIFS client (jane-2), against the real SMB root `/mnt/jane-nas/Entertainment/Audiobooks`. Method A needs no privilege; Method B needs `sudo` only to reset the CIFS counters.

Record the live mount options first, because the result is only meaningful tagged with them:

```shell
findmnt -no OPTIONS /mnt/jane-nas
```

Expect the production default: `vers=3.1.1,seal` with CIFS caching defaults (`cache=strict,actimeo=1`), and none of `cache=loose`, `actimeo=30`, `multichannel`, or `max_channels` (those were the reverted experiments). If the live mount still carries any of them, note it in the verdict; the numbers describe whatever the mount actually is.

Build the harness once as the normal user:

```shell
cargo build --release --example scan_bench
```

## Method A: what the scanner asks for

Trace one serial gaps walk and count the directory reads against the per-entry stat fallback. The stat family differs by libc, so trace all of its variants:

```shell
strace -f -c -e trace=getdents64,newfstatat,statx,lstat \
  target/release/examples/scan_bench \
  --root /mnt/jane-nas/Entertainment/Audiobooks \
  --mode gaps --iterations 1 --concurrency 1 --no-save --label roundtrip \
  2> /tmp/cifs-roundtrip-strace.txt
```

`scan_bench` runs one discarded warmup walk plus the measured iterations, so this is two full walks; the ratio is what matters, not the absolute count. Its stdout prints `dirs_visited` and `entries_seen` for the root; keep that line. The `-c` summary lands in the strace file.

- `getdents64` tracks directories, roughly two calls per directory (one returning entries, one returning the terminating zero).
- The stat-family row (`newfstatat`, `statx`, or `lstat`, whichever is large) is the per-entry `file_type()` fallback, made only when the dirent's `d_type` is `DT_UNKNOWN`.

Read with `--mode gaps` first, since that is the hot path a page load runs. Optionally repeat with `--mode all`, which does not prune covered subtrees and so visits more.

## Method B: what reaches the server

A stat syscall does not always hit the wire; CIFS can serve it from the attributes the directory query already cached. To see real round trips, zero the CIFS counters, run one scan, then read them:

```shell
sudo sh -c 'echo 0 > /proc/fs/cifs/Stats'
target/release/examples/scan_bench \
  --root /mnt/jane-nas/Entertainment/Audiobooks \
  --mode gaps --iterations 1 --concurrency 1 --no-save --label roundtrip
cat /proc/fs/cifs/Stats > /tmp/cifs-roundtrip-stats.txt
```

In the `\\...\pool` share section, two SMB2 command counts matter (the labels may be singular or plural by kernel):

- `QueryDirectory` / `QueryDirectories` tracks directories read.
- `QueryInfo` / `QueryInfos` tracks per-file attribute queries, the stat round trips.

Quiet other activity on the mount during the run, so nothing else inflates the counts.

## Reading the result

| Method A stat row | Method B `QueryInfo` delta | Verdict |
| ----------------- | -------------------------- | ------- |
| Near zero | Near zero | `d_type` is populated; the walk is already at the floor of one directory query per directory. Done, document the floor with confidence. |
| Tracks `entries_seen` | Near `QueryDirectory` (tracks dirs) | The code asks per entry, but CIFS serves it from the readdir cache. No wire amplification; a code tidy is optional, not a perf win. |
| Tracks `entries_seen` | Tracks `entries_seen` | Real per-entry round trips on the wire, the ~10x amplification. A scanner fix to stop calling `file_type()` per entry (trust the readdir type, or read raw `getdents64`) would cut cold scan time substantially. |

`seal` and the `actimeo=1` default both shape Method B; capture against the production mount so the numbers describe what users actually pay.

## Deliverables

- Save the raw outputs under `benchmarks/`, named by host and unix time: `cifs-roundtrip-strace-<host>-<time>.txt` and `cifs-roundtrip-stats-<host>-<time>.txt`. Redact server addresses, session ids, and serials the way `cifs-debugdata-jane-2-redacted.txt` does.
- Write a short verdict: the live mount options, the four numbers (`getdents64`, the stat-family count, `QueryDirectory` delta, `QueryInfo` delta) against the printed `dirs_visited` and `entries_seen`, and which row of the table this landed on.
- Report back. Do not change the scanner or the mount here; this is measurement only. A per-entry-amplification result becomes a scanner fix proposed separately; a floor-confirmed result graduates into the README "Network shares" guidance.
