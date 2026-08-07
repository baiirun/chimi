# Native Direct-Lowering Workloads

These sources force the first direct Cranelift lowering slice through the real
compiler and ABI-v2 runtime.

Build and run the representative pure-integer tree with:

```sh
cargo run -q -p chimi -- build \
  prototypes/native-direct-lowering/integer-arithmetic.chimi \
  -o target/direct-integer
target/direct-integer --trace=off
target/direct-integer --trace=full
```

`integer-arithmetic.chimi` covers integer constants, transparent grouping,
negation, addition, subtraction, and multiplication. Full tracing deliberately
uses the canonical runtime evaluator. `integer-overflow.chimi` proves checked
SSA falls back to the existing arithmetic defect rather than wrapping.
