# Native Build Cache Baseline

The first production `chimi build` implementation emitted independently
addressable artifacts but regenerated every one on every invocation. This
baseline verifies the content-addressed cache added by `prog show ts-d31a46`
and records its first edit/rebuild measurements.

## Cache contract

The cache stores immutable module objects, the entry object, whole-program
metadata, and the ABI-v1 runtime archive under the build root's shared
`target/chimi/.cache/native-v1` directory. Each identity includes the artifact
kind and cache schema so byte sequences from different boundaries cannot alias.

- Module objects include their verified Core nodes and origins, target CPU
  configuration, Cranelift version/settings, and native-lowering revision.
- The entry object includes the selected entry node and the same native target
  and lowering inputs.
- Metadata includes the exact generated source, runtime header, C compiler
  identity/settings, and target.
- The runtime archive includes its embedded C/header sources, runtime ABI,
  compiler/settings, archive mode, and target.

Every build still materializes the selected artifacts into its inspectable
artifact directory, performs the final platform link, and inspects dynamic
dependencies. Cache reuse therefore changes build work, not the executable or
deployment contract. `chimi.build.v1` reports ordered `cache.hits` and
`cache.misses` arrays.

## Correctness evidence

The CLI integration test performs a cold two-module build, repeats it unchanged,
then changes only the imported module's integer result. The unchanged build hits
all five artifacts. The edited build misses only the changed module object and
`program_data.o`, hits the unrelated module object, `entry.o`, and
`libchimi_rt.a`, and the linked executable returns the new value. Unit coverage
also proves length-framed cache inputs cannot alias and keeps the Rust and
embedded-C runtime ABI versions in agreement.

## Host measurements

Measured August 6, 2026 on Apple M1 (`arm64`, Darwin 25.5.0), Apple clang
21.0.0, and a release-built `chimi 0.0.0-bootstrap`. Each class contains 12
fresh compiler processes building the two-module CLI fixture. Wall time includes
project loading, compilation/cache lookup, artifact materialization, final link,
and dependency inspection.

| Build class | p50 | p95 | Mean |
| --- | ---: | ---: | ---: |
| Cold, isolated cache | 293.860 ms | 511.556 ms | 310.467 ms |
| Identical rebuild, all hits | 73.627 ms | 77.718 ms | 74.160 ms |
| Imported value edit, selective misses | 103.615 ms | 107.097 ms | 103.975 ms |

The identical-build p50 is 74.9% below the cold p50; the selective-edit p50 is
64.7% below cold. This small fixture is dominated by process startup, linking,
and dependency inspection after caching, so these values establish a regression
baseline rather than a large-project scaling claim.
