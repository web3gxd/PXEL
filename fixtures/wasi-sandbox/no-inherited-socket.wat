(module
  (import "wasi_snapshot_preview1" "sock_accept" (func $accept (param i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    i32.const 3 i32.const 0 i32.const 0 call $accept
    i32.const 8 i32.ne
    if unreachable end))
