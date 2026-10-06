(module
  (memory (export "memory") 1)
  (func (export "_start")
    i32.const 4096 memory.grow
    i32.const -1 i32.ne
    if unreachable end))
