# ADR-002: Stable row and column identities

Date: 2026-09-28. Status: ACCEPTED FOR PHASE 0 identity foundation; formula/structural semantics require their own phase gates.

## Context

A1 coordinates change after insertion/deletion. Using position as identity would attach delayed edits and dependencies to the wrong cell. Allocating every potential Excel cell would defeat sparse storage.

## Decision

Typed UUIDv7 identities for documents/sheets/rows/columns. Cell key is sheet+row+column identity. Axis uses ordered Vec IDs, ID→position index and visible extent; positions remain mutable. Start with 1000 rows and 26 columns and no Cell records. UI-triggered ExtendRows adds a bounded batch at the end as the user approaches the edge.

IDs are never reused. Sparse cell content/style records exist only when needed. Formula bound targets follow stable identities; absolute/relative modes govern copying independently of structural movement. Source/diagnostic AST and persisted normalized references are separate types.

## Alternatives

Coordinates as keys break delayed operations. Allocating 1M row IDs on every empty sheet wastes memory/snapshot space. Order-statistics trees or sequence CRDTs may help larger workloads but need measured evidence before adoption.

## Consequences

Axis insertion shifts vector/index entries; accepted for the bounded MVP until benchmark proves otherwise. Increasing extent is a real versioned structural operation with automatic UX. Sorting a rectangle cannot be implemented by moving full-row identity: early SortRange is deferred, future SortSheet is separate.

## Validation

Stable IDs survive insertion/reload; deleted targets produce typed errors; memory and snapshot size of 1k/100k axes are measured. Copy/move/cut/reference fixtures are required before declaring those semantics compatible.
