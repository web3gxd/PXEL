# Sparse-style local Criterion sample

Date: 2026-10-02. Host: Windows 11, Intel Core Ultra 9 185H, 16 GiB RAM.
Build: Rust release/optimized profile; Criterion 10 samples, 0.2 s warm-up and
1 s measurement per case. Command:

```powershell
$env:CARGO_TARGET_DIR='.local/target-style-memory'
& .tools/cargo/bin/cargo.exe bench --locked -p spreadsheet-domain --bench style -- --warm-up-time 0.2 --measurement-time 1 --sample-size 10
```

| Case                                   | Criterion interval |
| -------------------------------------- | -----------------: |
| Apply one style leaf                   |   14.283–14.787 ns |
| Apply all 16 style leaves              |   61.752–68.134 ns |
| Canonical encode one rich style        |   44.214–60.893 ns |
| Build 100,000 entry sparse style map   |   14.601–18.261 ms |
| Canonical encode 100,000 sparse styles |   2.7624–3.1812 ms |

The fixture reported `size_of::<CellStyle>() = 64` bytes and
`size_of::<CellAddress>() = 48` bytes. Those field values alone account for a
11.2 MB lower bound at 100,000 entries (6.4 MB styles + 4.8 MB addresses).

## Process-memory probe

A separate release probe held 100,000 stable addresses for three seconds, then
held the same address vector plus a `BTreeMap<CellAddress, CellStyle>` with
100,000 entries for three seconds. A third mode cycles four full styles across
the same number of entries, including all 16 properties and four common font
families. The same executable and machine were used. After the probe printed
`ready` with allocations complete, PowerShell sampled
`PrivateMemorySize64` and `WorkingSet64` 100 times at 10 ms intervals. Three
fresh processes were measured per mode; table cells show the median of the
three per-process sample medians and maximum sampled values. This avoids
Criterion retaining its benchmark fixture and a transient rebuilt map
simultaneously.

The fixture is versioned at
[`style_memory_probe.rs`](../../crates/spreadsheet-domain/examples/style_memory_probe.rs).
Rebuild it in release mode before collecting process samples:

```powershell
. .\scripts\env.ps1
$env:CARGO_TARGET_DIR = '.local/target-style-memory'
$env:CARGO_INCREMENTAL = '0'
cargo build --locked --release -p spreadsheet-domain --example style_memory_probe
```

| Mode                               | Median private bytes after ready | Max sampled private bytes | Median working set after ready | Max sampled working set |
| ---------------------------------- | -------------------------------: | ------------------------: | -----------------------------: | ----------------------: |
| Addresses only                     |                         8.11 MiB |                  8.11 MiB |                       9.57 MiB |                9.57 MiB |
| Addresses + 100k compact styles    |                        18.46 MiB |                 18.47 MiB |                      19.57 MiB |               19.59 MiB |
| Addresses + 100k mixed full styles |                        20.46 MiB |                 20.46 MiB |                      21.54 MiB |               21.54 MiB |

The incremental private-byte cost over the same address vector is 10.35 MiB for
compact styles and 12.35 MiB for mixed full styles. These are process-wide
Windows measurements, not allocator-attributed heap measurements; they include
the Rust runtime, executable mappings and allocator behavior. The probe models
the actual domain map and per-style font allocation but not a full workbook,
server, browser, PostgreSQL, or concurrent clients. It is a reproducible local
memory profile, not a production budget or proof of production memory safety.

## Windows reference budget

Added `scripts/verify-style-memory.ps1` to rebuild this probe, start three fresh
processes per mode, sample private bytes and working set 100 times per run, and
fail if the median incremental private bytes for 100,000 mixed full styles exceed
16 MiB over the address-only mode. The 16 MiB reference allows about 29% headroom
over the initial measured 12.35 MiB incremental cost. It applies only to this
pinned Windows native-domain fixture; it is not a Linux or server-wide memory
limit.

The script passed on 2026-10-02. Three axis-only processes had median private
bytes of 8.09 MiB; three rich-style processes had 20.49 MiB. Median incremental
private bytes were 12.40 MiB, under the 16 MiB reference budget. Working sets
were 9.54 MiB and 21.55 MiB respectively. Full per-run samples are captured by
the script's JSON output; execution command and host details are in RUNBOOK.
The representative full-workbook/server/DB and Linux stress gate remains open.

Criterion marked outliers in the 16-leaf, map-build and full-map encode cases.
This was a local laptop run without CPU isolation. Treat the intervals as a
preliminary baseline, not a controlled regression threshold or production p95.
No storage representation change was made from this evidence alone.
