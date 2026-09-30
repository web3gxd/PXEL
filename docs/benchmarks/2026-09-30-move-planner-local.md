# Frozen cut/move planning

Measured 2026-09-30 on Windows 10.0.26100, Intel Core Ultra 9 185H,
15.61 GiB visible RAM, pinned Rust 1.98.1. The developer machine also ran the
local app and PostgreSQL; concurrent load was uncontrolled.

```sh
cargo bench -p spreadsheet-formula --bench relocation -- --sample-size 10 --warm-up-time 1 --measurement-time 2
```

The optimized fixture has 6000 allocated rows and 3 columns, 1000 numeric
source cells and 5000 external formulas referencing the source. Moving A1:A1000
to B1:B1000 prepares exactly 7000 unique source/destination/inbound writes with
preimages and input hash. Fixture setup is excluded; the complete planner is timed.

Criterion first reported **8.3701–9.6218 ms**, central estimate **8.9449 ms**,
10 samples, 1 s warm-up, 2 s requested measurement (275 iterations).
Review then removed clone/discard of independent formulas via borrowed AST
inspection before rewriting only affected formulas. The first post-change inbound
run was slower: **9.1744–12.493 ms**, central **10.101 ms**. A repeat prompted by
that regression reported **8.4866–8.8520 ms**, central **8.6696 ms** (220 iterations).
The variable uncontrolled runs do not establish a general speedup or regression.

A second fixture retains the 1000 moved numeric cells but its 5000 external
formulas are independent `=42`; exactly 2000 addresses are planned. Before
borrowed inspection: **3.5623–4.1173 ms**, central **3.8243 ms** (550 iterations).
Afterwards: **2.2454–2.3825 ms**, central **2.3117 ms** (770 iterations); repeat
**2.3450–2.5138 ms**, central **2.4028 ms** (825 iterations). The implementation
avoids rebuilding those 5000 independent formulas. This local improvement is not
a controlled production performance promise. All runs use the same sample/
warm-up/measurement settings; timing intervals are not p95 UI or commit latency.
No database, ACL/history transaction, candidate application, formula evaluation,
network, serialization or browser/WASM crossing is included. No production
performance gate is closed by this developer-machine result.
