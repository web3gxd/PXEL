(module
  (import "wasi_snapshot_preview1" "environ_sizes_get" (func $env (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    i32.const 0 i32.const 4 call $env
    if unreachable end
    i32.const 0 i32.load
    if unreachable end))
