(module
  (import "wasi_snapshot_preview1" "sock_open" (func $socket (param i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    i32.const 2 i32.const 1 i32.const 0 call $socket drop))
