# Native Runtime Prebuild (ts-1db9f0)

A cold `chimi build` compiled `compiler/chimi/runtime/native/runtime.c` at
`-O2` into every new project's cache. On the two-module build-cache fixture that
compile was 663 ms of a 791 ms cold build. Since `ca59823` the compiler's build
script compiles the runtime archive once, at `cargo build` time, and the
compiler embeds it. `chimi build` links the embedded archive when its key
matches the runtime sources the compiler embeds and the `cc` it finds now;
otherwise it compiles the runtime into the project cache as before. The build
report's `cache.prebuilt` list says which path a build took.

## Result

| Build | Before (`d8feefa`) p50 | After (`ca59823`) p50 | Change |
| --- | ---: | ---: | ---: |
| Cold, empty project cache, run 1 (load 32.6) | 3,316 ms | 280 ms | -92% |
| Cold, empty project cache, run 2 (load 27.6) | 2,853 ms | 309 ms | -89% |
| Cold, empty project cache, run 3 (load 23.4) | 2,048 ms | 249 ms | -88% |

**Both runs are noisy.** Other agents were building the whole time and the
1-minute load average never fell below 22 during this work, against 4.8 to 5.2
for the Phase 1 numbers (768 ms cold before this change). The ratio is the
useful figure: at equal load, removing the runtime compile cuts a cold build by
about 90%. The fastest "after" build in run 3 took 119 ms, and the fastest
"before" build 856 ms. The ~130 ms target from the Phase 1 analysis (791 ms minus the
663 ms compile) was not confirmed at low load; re-run the command below when
the machine is quiet and add a row.

Warm builds are unchanged by design: a warm build already hit the cached
runtime archive, and now copies the embedded one instead.

The compiler binary grows by the archive, about 136 KB (4,776,640 B to
4,925,248 B, release, macOS arm64). The prebuild adds one `runtime.c` compile
to a `cargo build` of the `chimi` crate whenever the runtime sources or
`build.rs` change.

## Method

- Host: Apple M1 (MacBookPro17,1), 8 logical cores, macOS 26 (Darwin 25.6.0),
  Apple clang 21.0.0 as `cc`.
- Binaries: `cargo build --release -p chimi --bins` with a private
  `CARGO_TARGET_DIR`; "before" from a `git archive` export of `d8feefa`,
  "after" from `ca59823`.
- Fixture: the two-module project from the `build-cache` test in
  `compiler/chimi/tests/cli.rs` (`app/main.chimi` imports `lib/answer.chimi`).
- Each run builds from a new empty working directory, so the project cache
  (`<cwd>/target/chimi/.cache/native-v1`) is empty; before and after
  alternate; 9 (run 1) and 11 (runs 2 and 3) pairs; median wall time of the
  `chimi build` process. The load column is the median 1-minute load average
  sampled before each build.

Save this as `measure.py` outside the repository and run
`python3 -I measure.py "$P" 11`, where `$P` holds `chimi-before`,
`chimi-after` and `fixture/project`:

```python
import os, subprocess, sys, time, tempfile, shutil, statistics
S = sys.argv[1]; runs = int(sys.argv[2])
fixture = os.path.join(S, "fixture/project")
result = {"before": [], "after": []}
loads = []
for i in range(runs):
    for name in ("before", "after"):
        work = tempfile.mkdtemp(dir=S, prefix="cold-")
        loads.append(os.getloadavg()[0])
        t = time.perf_counter()
        subprocess.run([os.path.join(S, "chimi-" + name), "build", "--root", fixture, "app/main", "-o", os.path.join(work, "cached-app")], cwd=work, check=True, stdout=subprocess.DEVNULL)
        result[name].append((time.perf_counter() - t) * 1000)
        shutil.rmtree(work)
for name, values in result.items():
    print(name, "p50 %.0f ms  min %.0f  max %.0f  n=%d" % (statistics.median(values), min(values), max(values), len(values)))
print("load 1-min median %.1f (min %.1f, max %.1f)" % (statistics.median(loads), min(loads), max(loads)))
```

## Fallback

The per-project path still exists and is tested
(`a_c_compiler_unlike_the_one_that_prebuilt_the_runtime_compiles_it_per_project`
in `compiler/chimi/tests/cli.rs`). A build takes it, and pays the old cold cost
once per project, when:

- `cc --version` differs from the compiler's build (a toolchain update);
- the compiler was built without a prebuilt archive: the `native-backend`
  feature off, a cross-compile (`HOST != TARGET`), no `cc`, or a runtime that
  did not compile (the build script prints a cargo warning);
- the runtime sources `build.rs` hashed differ from the ones `native.rs`
  embedded, which can only happen if they changed during one cargo build.
