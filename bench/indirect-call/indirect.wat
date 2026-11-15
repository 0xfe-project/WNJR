;; call_indirect microbench for the generic dispatch path.
;; Exported table plus a table.set (has_table_mutation) => TableDispatchMode::Generic,
;; so every call goes through the self-absolute range block. 4 rotating callees.
(module
  (type $ft (func (param i64) (result i64)))
  (table (export "t") 4 funcref)
  (func $add1 (type $ft) local.get 0 i64.const 1 i64.add)
  (func $add2 (type $ft) local.get 0 i64.const 2 i64.add)
  (func $mul3 (type $ft) local.get 0 i64.const 3 i64.mul)
  (func $xor (type $ft) local.get 0 i64.const 0xdeadbeef i64.xor)
  (elem (i32.const 0) $add1 $add2 $mul3 $xor)
  (func (export "set") (param i32 funcref) local.get 0 local.get 1 table.set)
  (func (export "bench") (param $n i64) (result i64)
    (local $i i64) (local $acc i64)
    (block $done
      local.get $n i64.eqz br_if $done
      (loop $L
        local.get $acc
        local.get $i i32.wrap_i64 i32.const 3 i32.and
        call_indirect (type $ft)
        local.set $acc
        local.get $i i64.const 1 i64.add local.tee $i
        local.get $n i64.lt_u
        br_if $L))
    local.get $acc))
