# ADR-005: Viewport Canvas grid with DOM editing overlay

Date: 2026-09-28. Status: ACCEPTED FOR PHASE 0 rendering foundation.

## Context

The editor must open sparse sheets with 100 000 populated cells without creating one DOM element per cell. Keyboard input, text selection/IME and accessibility still benefit from browser-native controls.

## Decision

Canvas 2D draws the visible viewport plus overscan, respecting devicePixelRatio. requestAnimationFrame coalesces resize/scroll/pointer work. React manages chrome and overlays; sparse document state stays outside per-cell React state. A DOM editor/formula bar share a future editing state machine; bounded accessibility mirror supplies active/visible semantics.

Phase 0 renders an empty grid and connection status. Selection/editing/clipboard/optimistic updates begin in later vertical slices. Web Worker handles WASM and heavy work when those features exist. Caret blinking belongs to the DOM editor and does not require repainting the whole grid.

## Alternatives

DOM table/full React cells violate scalability requirements. Ready-made grid frameworks are prohibited. WebGL and multilayer canvas may become useful, but add complexity without current profiling evidence.

## Consequences

Hit testing, selection, size indexes and accessibility need explicit modules. Native DOM editing handles IME/clipboard. Static/dynamic layers or dirty-region optimizations require benchmarks and correctness tests before introduction.

## Validation

Real browser tests at DPR 1/2 check drawing, resizing, keyboard focus and bounded DOM nodes. First usable render is measured; shared CI does not enforce a universal hard latency threshold. 100k populated cells becomes a phase 1 benchmark, not a fabricated phase 0 test.
