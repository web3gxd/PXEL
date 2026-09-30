# Cell style core: local baseline, 2026-09-30

Scope: pure Rust CellStyle application/canonical encoding and a standalone
BTreeMap of100000 typed stable addresses. This is not a Workbook, formula
calculation, Canvas render, optimistic edit, PostgreSQL transaction or network
benchmark. No implementation optimization was made from this run.

Machine: Intel Core Ultra9 185H, Windows10.0.26100, physical RAM15.61GiB,
rustc1.98.1 (48a229cea,2026-09-01), optimized bench profile. The local development
server/PostgreSQL remained running; no HTTP/browser regression was running during
this benchmark. This developer machine is not the controlled acceptance runner.

```powershell
. ./scripts/env.ps1
cargo bench --locked -p spreadsheet-domain --bench style -- --sample-size 10 --measurement-time 1 --warm-up-time 1
```

The command passed; release compilation took37.01s. Criterion collected10 samples
per case after a1s warm-up. Its printed lower/central/upper time estimates were:

| Case                                       |    Lower |  Central |    Upper |
| ------------------------------------------ | -------: | -------: | -------: |
| Apply one leaf to default style            | 25.090ns | 32.184ns | 36.312ns |
| Apply all16 leaves to default style        | 96.135ns | 97.891ns | 99.294ns |
| Canonical rich style, reused output buffer | 35.208ns | 36.121ns | 37.900ns |
| Build/drop100000-entry sparse map          | 14.370ms | 14.940ms | 15.438ms |
| Canonical100000-entry map, reused buffer   | 2.7569ms | 2.8488ms | 2.9602ms |

These estimates are not p95/p99. One rich-encoding sample was a severe high
outlier; one map-encoding sample was a mild high outlier. Ten samples and a short
measurement window establish a preliminary baseline, not a regression threshold.

Single-leaf input sets bold. Rich input explicitly changes all16 properties,
including a9-byte font name and four identical border edges. The large map uses
bold plus fill on every address and one shared sheet/column. Addresses are
constructed outside timing. Map construction includes collection/cloning and
destruction; encoding includes row/column bytes and each canonical style payload,
but no document envelope, calculation metadata or hashing.

Actual layout: CellStyle value64bytes; CellAddress48bytes. The100000 values and
keys therefore account for11.2MB before BTree nodes, allocator overhead, map
capacity and temporary buffers. This is a layout count, not measured RSS or an
allocation bound. The proposed512byte/entry admission charge remains a policy
budget and must not be presented as measured heap memory.

Next measurements must cover real Workbook content/style union, candidate/hash,
guarded inverses, snapshot4 and actual Worker/Canvas boundaries. Whole-book
initialization and incremental updates require separate cases. Existing optimistic
16ms and durable commit50ms targets are neither passed nor contradicted by these
isolated core measurements.
