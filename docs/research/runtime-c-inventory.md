# What is left in runtime.c (CHI-305 step 5 inventory)

Taken at main `b6f94a33` (2026-11-26). Paths are under `compiler/chimi/runtime/native/` unless stated. `runtime.c` has 11,778 lines; the ranges below are contiguous and add up to that. (Line 1's "ABI version 22" is stale; `runtime.h:8` and `native.rs:34` say 60.)

## 1. Subsystems in file order

Categories: H = heap/allocator, C = collector, V = value representation and builders, I = intrinsic implementations, D = invoke/dispatch, B = binding call path, T = tracing/telemetry, P = startup/exit/process glue, G = C glue around Rust, O = other.

| Lines | # | Subsystem | Cat |
|---|---:|---|---|
| 1–137 | 137 | Includes, weak `chimi_fiber_*` fallbacks (61–90), `ValueKind`/`CallableKind`/`ControlKind` enums | O/V |
| 138–273 | 136 | `ChimiValue` (191), list/map nodes, `ChimiCallable`, environment, control, task context, `AllocationKind` | V |
| 274–373 | 100 | `struct ChimiRuntime` (flags, trace files, signal injection) | P |
| 374–541 | 168 | Heap model comment, `AllocationHeader`/`YoungChunk`/`Area`/`Scope`/`RuntimeHeap`, `main_heap`, `static_area`, epochs, profile rows, `runtime_exit` | H |
| 542–1003 | 462 | Small-block cache, `Carrier` struct (598–674) and its TLS access (733–757), footprint counters | H |
| 1004–1192 | 189 | `area_link`, young chunk take/give, `flush_young_counts`, `young_bump`, **`allocate` (1100)**, `allocation_kind` | H |
| 1193–1292 | 100 | Handle counts: `handle_count_lock` around Rust `chimi_handle_counts_*_v1` | G |
| 1293–1395 | 103 | `release_allocation`, area release and merge, fragment record, `heap_create`/`heap_destroy`, `new_value`/`new_control`/`success`/`defect` | H |
| 1396–1483 | 88 | Host stack guard (`/proc/self/maps`), `default_carriers`, carrier thread entry | P |
| 1484–1573 | 90 | Civil date math, timestamp text and parsing | I |
| 1574–1845 | 272 | Environments, closure capture, persistent list vector, `list_slice` | V |
| 1846–2141 | 296 | `mark_push`/`mark_children`, `forward_young`/`forward_children`/`forward_survivors`, `reset_young_chunks` | C |
| 2142–2283 | 142 | Regions: `allocation_scope_begin/sweep/collect/end`, promote, sweep, `take_mark_epoch` | C |
| 2284–2627 | 344 | Fragment copy: copy table, `copy_forward`, `copy_scan`, `fragment_copy/adopt/duplicate/free` | C |
| 2628–2825 | 198 | `run_node` (2641), telemetry JSON (`emit_event` 2694, `trace_lock`), profile, spans, `begin_node`/`finish_node` | T/D |
| 2826–3093 | 268 | Scalar and text builders, shared Void/True/False (2837), arithmetic, decimal ABI glue, callable/map/task/channel builders | V |
| 3094–3347 | 254 | `scheduler_request` (3108), capability transfer walk for `Task.start` and sends | C/G |
| 3348–3362 | 15 | `chimi_rt_full_trace`, `chimi_rt_direct_integer_success` | D |
| 3363–3908 | 546 | AVL map, structural compare (3780), JSON-number canonicalisation, JSON value walk | V/I |
| 3909–4121 | 213 | `TextBuffer`, JSON encode, Rust JSON tape to values (`json_build` 4056) | I |
| 4122–4301 | 180 | `invoke_core_operation` (text and list core, Rust text calls) | I |
| 4302–4513 | 212 | Bytes intrinsics | I |
| 4514–4774 | 261 | Assertions, `capture_defect`, test logs (lock, fragments) | I |
| 4775–5077 | 303 | stdin `ByteInput`, `wait_readable`, `process_utf8_lines` (a region loop) | I/C |
| 5078–5148 | 71 | Random: `random_lock` around the Rust generator, OS seed | G |
| 5149–5341 | 193 | Console: `console_lock`, queue and drain around the Rust console | G |
| 5342–5419 | 78 | `invoke_host`, std/log | I |
| 5420–5526 | 107 | Metrics: `metrics_lock` around the Rust metrics | G |
| 5527–6006 | 480 | `ChimiBindingCall`, the Rust handle table glue (`binding_handle_lock`), argument readers, value and result builders, fd waits, `invoke_binding` | B |
| 6007–6057 | 51 | **`invoke` (6013)** | D |
| 6058–6179 | 122 | Task fiber entry (6107: heap per task), scheduler release and copy hooks (6155–6172) | G/C |
| 6180–6320 | 141 | Int and Float parsing, Duration glue | I |
| 6321–6695 | 375 | `Debug.render` | I |
| 6696–7017 | 322 | Causes (build, link, render, intrinsics) | I/V |
| 7018–7191 | 174 | Test mailbox, process data values | I |
| 7192–7752 | 561 | Process kernel: `chimi_rt_process_entry` (7212), spec marshalling, `invoke_process_kernel` | G (step 3) |
| 7753–8020 | 268 | Map, Set, JSON and Decimal intrinsic cores | I |
| 8021–8359 | 339 | Clock, timeouts, `resource_scoped` | I |
| 8360–8609 | 250 | `start_task`, combinators (all, results, race) | I |
| 8610–8842 | 233 | `select` / Wait | I |
| 8843–8901 | 59 | env host operations | I |
| 8902–9290 | 389 | fs: syscalls plus `fs_file_lock` around the Rust files table | I/G |
| 9291–9804 | 514 | **`invoke_intrinsic` (9291)**: range checks, then a switch | D |
| 9805–9977 | 173 | Constant decoding into the static area, `eval_reference` | V/P |
| 9978–10468 | 491 | `NODE_WRAPPER` and the 18 `chimi_rt_*` nodes, match, `run_loop_passes` (10381), `chimi_rt_direct_loop` (10437) | D |
| 10469–10668 | 200 | Outcome printing, profile and memory reports | T |
| 10669–11050 | 382 | Scheduler event trace, capability violation, deadlock and defect reports | T |
| 11051–11293 | 243 | `program_exit`, stop, user and child signals, `chimi_binding_wait_child` | P |
| 11294–11778 | 485 | `execute_program_entry`, `chimi_rt_main` (11395): CLI parsing and scheduler config | P |

Rough totals: H about 920 lines, C about 1,300 (with transfer), V about 1,500, I about 4,400, D about 1,070, B 480, T about 780, P about 1,400, G about 1,300 (process kernel included).

## 2. ABI surface the generated code uses

The Cranelift objects come from `compiler/chimi/src/backend/native.rs`. They import the following:
- **Node helpers**, `ChimiControl *(ChimiRuntime *, u32 node, ChimiEnvironment *, u64 span)`: `chimi_rt_{constant, reference, select, dependency_method, construct, call, list, tuple, record, unary, binary, identity, provision, sequence, closure, match, loop, break}` (native.rs:1295–1324; signature at 1242).
- **Fast paths**: `chimi_rt_full_trace` (1327), `chimi_rt_direct_integer_success(rt, u32, i64)` (1334), `chimi_rt_direct_loop(..., ChimiDirectPass)` (1352).
- **Entry**: `chimi_rt_main(entry, argc, argv)` (1413). The objects export `main`, `chimi_node_<id>` (1287; node signature at 1229) and local `chimi_loop_pass_<id>` (1547).

The program data is C source that `cc` compiles against `runtime.h` (`backend/program_data.rs:507`). It defines `CHIMI_COMPUTATION`, `CHIMI_TRACE_MODULES`, `CHIMI_REQUIRES_SCHEDULER`, `CHIMI_STRINGS`, `CHIMI_STRING_LENGTHS`, `CHIMI_IDS`, `CHIMI_ORIGINS`, `CHIMI_NODES`, `CHIMI_SYMBOLS`, `CHIMI_FUNCTIONS`, `CHIMI_CONSTRUCTORS`, `CHIMI_NAMED_NODES`, `CHIMI_STEPS`, `CHIMI_ARMS`, `CHIMI_PATTERNS`, `CHIMI_BINDINGS` and an extern for each binding function (585), `CHIMI_CONSTANT_VALUES`, the test and assertion tables, the `*_COUNT`/slot constants, and `CHIMI_NODE_FUNCTIONS` (761). (`CHIMI_OPCODES`/`chimi_bytecode_eval`, 724–745, are bytecode-prototype only.)

Other symbols:
- **Binding shims** call `chimi_binding_*` (`chimi_binding.h:59–141`). Their types are opaque (`ChimiBindingCall`, `Value`, `Result`, at 44–46).
- `process_lab/heap_kind.rs:37` looks for the symbol `chimi_rt_process_entry`.
- The archives `libchimi_rt.a` and `libchimi_rt_process.a` are named at native.rs:287–289 and 646–651.

**Inlining: none.** Generated code never loads or stores a field of `ChimiValue`, `ChimiControl`, `ChimiRuntime` or `ChimiEnvironment`, and has no allocation fast path. The only memory operation is one store of an `i64` into the pass's `out` pointer (`direct_loop.rs:231`). Direct integer roots compute in registers and box through `chimi_rt_direct_integer_success`. **So the codegen does not freeze `ChimiValue`'s layout, `ValueKind` tags or the heap header.**

These are frozen, because generated code or data hardcodes them:
- Pass codes 0, 1 and 2 (`direct_loop.rs:26–28`, matching `runtime.h` `CHIMI_DIRECT_PASS_*`).
- Symbol kinds (`program_data.rs:872`), constant kinds as literals (`program_data.rs:58ff`, for example 16 and 18), pattern kinds (184ff), binary operators (1092) and opcodes (815).
- `StandardType`, `StandardFunction` and `StandardConstructor::runtime_id() = self as u32` (`standard.rs:374, 1353, 1769`). These must keep the `CHIMI_TYPE_*`, `CHIMI_INTRINSIC_*` and `CHIMI_CONSTRUCTOR_*` order.
- The `#[repr(C)]` layout of the program-data structs in `runtime.h:414–553`.

## 3. Hot paths

- **Every allocation.** Callers use `new_value` (1380), `new_control` (1384) or `new_value_slots`/`new_environment` (1574–1593). These go to `allocate` (1100), which reads `carrier` once through the TLS asm (746–751). Inside a region whose `!links_young` holds, an allocation of at most `YOUNG_CHUNK_LARGEST` (16 KiB chunk / 4) goes through `young_rounded` (1096) to `young_bump` (1074), a pointer bump. On refill, `young_chunk_take` (1013) uses a per-carrier cache of 16 or `calloc`, then `footprint_fold`. Otherwise `current_area` (851), `small_block_take` (815), `area_link` (1004) and `footprint_add` run. Typed objects then call `allocation_kind` (1155). There is one `profile_rows` branch per allocation.
- **Every node.** `chimi_node_N` calls `chimi_rt_X`, built by `NODE_WRAPPER` (9978): `profile_enter`, `begin_node` (span unless silent), body, `finish_node`. Children run through `run_node` (2641): a cancel check and an indirect call through `CHIMI_NODE_FUNCTIONS`. Every result allocates a `ChimiControl` (`success`, 1391), usually with a `ChimiValue`.
- **Every intrinsic call.** `chimi_rt_call` (10087) allocates the argument array, then calls `invoke` (6013), then `invoke_intrinsic` (9291), which tests the intrinsic against id ranges in sequence, then the matching `invoke_*_operation`, its builders and `allocate`. Bindings go from `invoke` to `invoke_binding` (5991).
- **Loop pass boundary.** `run_loop_passes` (10381) calls `allocation_scope_collect(&passes, &state, NULL)` (2243), then `allocation_scope_sweep` (2210): `take_mark_epoch` (2197), `flush_young_counts` (1055), mark (1862/1879), `forward_survivors` (2085), `reset_young_chunks` (2097), `promote_marked` (2155), and on a full pass `sweep_survivors`. On exit, `end_measured_allocation_scope` (4844) calls `allocation_scope_end` (2258).
- `chimi_rt_direct_loop` (10437) allocates nothing per pass.

## 4. Cross-dependencies that constrain order

- **Roots are explicit.** The collector marks only the `value` and `control` passed by address (2210–2224) and never scans the C or fiber stacks. Soundness rests on the invariants at 343–372 (immutable values that never point to younger ones; C frames hold current-pass pointers only via the passed roots). runtime.c never calls `chimi_fiber_visit_roots` or `ChimiRootFrame` (`chimi-fiber/src/lib.rs:51, 359`); it passes NULL roots at 3096. A Rust heap must keep the same discipline, or every intrinsic would need rooting.
- **Heap per fiber.** `Carrier.fiber = {heap, target}` (572–575). `scheduler_request` (3108) saves and restores it on every park. A task's heap is created and destroyed in `chimi_task_entry` (6117/6137) and adopts its captures fragment (6121). A process incarnation's heap is handled the same way (7217–7262). Fragments are transferred across these heaps by `fragment_copy` and `fragment_adopt` (4674, 6082, 7030, 7328, 7458, 7503, 7549, 7633).
- **The Rust scheduler's view.** `chimi-fiber` has no notion of a heap: grepping `src/` finds no heap or fragment. It holds opaque host words (fragment pointers as `u64` messages, task contexts as `void *`), and releases or copies them through hooks registered at runtime.c:11303–11328 (`release_task_word`, `release_channel_word`, `copy_channel_word` at 6155–6172, and `release_message_word`/`release_spec_context_word` at 7267–7274; see `chimi_fiber.h:687–747`). So the `chimi_fiber.h` boundary can stay as it is while the heap moves.
- **Shared values.** `static_area` (501) holds the decoded constants (9805+) and the shared Void/True/False (2837). They are never collected or copied. `list_shared_across_threads` (1807) keeps `list_slice` from compacting a constant while carriers run on threads. Mark epochs (505) and heap owners (500) are process-wide atomics.
- **The collector depends on handles.** `release_allocation` (1293) calls `counted_handle_drop`, which reaches Rust `chimi_handle_counts_drop_v1` under `handle_count_lock`, or `chimi_fiber_handle_drop`. A native handle at zero calls `native_handle_unreferenced` (5632), which closes binding handles. `copy_forward` (2354) retains them.
- **Value layout is known to everything in C**: marking (1879), forwarding (2002), `copy_scan` (2412), `transfer_visit` (3156), compare (3780), debug (6470), every intrinsic and the binding readers (5742–5862). Hence B and V move with the heap.
- **Dispatch depends on tracing**: `NODE_WRAPPER` and `allocate` read `profile_rows` and `runtime->silent`/`full_trace`, and `emit_event` takes `trace_lock`.
- **Tests depend on the C internals**: 8 of the 9 C fixtures in `compiler/chimi/tests/fixtures/` (for example `native-heap.c` and `native-list-fuzz.c`) `#include "runtime.c"`. Each needs a Rust replacement before deletion.

## 5. C outside the unit that could move first

1. **Signals and child reaping** (11051–11293): `sigaction`, self-pipes and `pthread_once`, with almost no `ChimiValue` use. The Rust scheduler already reads the signal words.
2. **Host stack guard and `default_carriers`** (1396–1483): `/proc/self/maps` and affinity syscalls, no values.
3. **Civil date and timestamp text and parsing** (1484–1573): pure arithmetic and parsing that can be fuzzed against C, a step 2 leaf that was missed.
4. **The locks around Rust tables**: `handle_count_lock` (1205), `random_lock` (5089), `metrics_lock` (5428), `fs_file_lock` (8959) and `binding_handle_lock`/`cond` (5579–5581). Moving each mutex into its crate removes C that exists only to serialise Rust. `console_lock` (5184) is harder: `console_lock_held` lives in `Carrier` and the lock spans parks.
5. **The stdin byte reader** (4866–4968: `ByteInput`, `input_next`, `wait_readable`). It deals only in bytes; the region loop in `process_utf8_lines` stays.
6. **Report writers** (10469–11050): the memory and profile reports and the scheduler, violation and deadlock text. These are JSON built from counters; they need a plain counter-snapshot struct first.
7. **CLI parsing in `chimi_rt_main`** (11395–11778): it could fill a `#[repr(C)]` config, leaving C to build `ChimiRuntime`.
8. **fs syscalls** (`fs_list`, `fs_metadata`, `fs_read`, `fs_write`, temporary directory; 9020–9175): these can return bytes or records to the C builders.
