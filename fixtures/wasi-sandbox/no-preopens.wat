(module
  (import "wasi_snapshot_preview1" "fd_prestat_get" (func $prestat (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    i32.const 3 i32.const 0 call $prestat
    i32.const 8 i32.ne
    if unreachable end))
