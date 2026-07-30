# macOS Sampling Profiler for Makepad Studio

## Short answer

If by "real profiler" we mean:

- out-of-process sampling
- attach to or launch a target process
- capture thread stacks at a fixed interval
- collapse stacks into folded format
- render a flamegraph
- drive it from Makepad Studio

then this is feasible in Rust, but it is not a tiny feature.

My estimate:

- `2-4 days`: wrapper around Apple tooling such as `sample` or `xctrace`
- `1-2 weeks`: native Rust MVP for Studio-launched debug builds, frame-pointer unwind only, SVG flamegraph export
- `4-8 weeks`: robust product-quality profiler with good symbolication, Apple-silicon edge cases, attach UX, cancellation, tests, and a proper Studio flamegraph tab

The key point is that the current Studio profiler is not the right substrate. It is an in-process time-series profiler. A real flamegraph profiler wants hierarchical stack samples.

## Why the current Studio profiler is the wrong shape

Today Studio's profiler protocol is built around timestamped sample arrays:

- `QueryProfiler` requests a `build_id`, `sample_type`, time range, and sample cap in [platform/studio/src/hub_protocol.rs](/Users/admin/makepad/makepad/platform/studio/src/hub_protocol.rs:246)
- `QueryProfilerResults` returns `event_samples`, `gpu_samples`, and `gc_samples` in [platform/studio/src/hub_protocol.rs](/Users/admin/makepad/makepad/platform/studio/src/hub_protocol.rs:459)
- the hub store only keeps those three linear sample streams in [studio/hub/src/log_store.rs](/Users/admin/makepad/makepad/studio/hub/src/log_store.rs:131)
- the desktop view renders charts over time in [studio/desktop/src/desktop_profiler_view.rs](/Users/admin/makepad/makepad/studio/desktop/src/desktop_profiler_view.rs:952)

A flamegraph is not a time-series chart. It is an aggregate view over stacks. Trying to cram it into the existing profiler query path would be awkward and would fight the current data model.

## What "real sampling profiler" means on macOS

Apple's `sample(1)` describes the behavior we want: it suspends the process at intervals, records call stacks for all threads, resumes the process, and later summarizes the results.

For our own implementation, the low-level building blocks are there:

- `task_for_pid(...)` in `mach_traps.h` gets the task port for a PID
- `task_threads(...)` in `mach/task.h` enumerates threads
- `thread_get_state(...)` in `mach/thread_act.h` reads registers
- `mach_vm_read_overwrite(...)` in `mach/mach_vm.h` reads remote memory
- `task_suspend(...)` / `task_resume(...)` and `thread_suspend(...)` / `thread_resume(...)` exist for consistent snapshots
- `TASK_DYLD_INFO` and `dyld_all_image_infos` expose loaded image metadata for symbolication

In other words: the collector itself is straightforward enough. The expensive parts are unwinding and symbolication.

## The hard parts

### 1. Permissions and `task_for_pid`

This is the first real constraint.

`taskgated(8)` is the access-control daemon for `task_for_pid`. Apple's debugger entitlement docs are also explicit:

- a debugger-style tool may request task ports
- this is tied to `com.apple.security.cs.debugger`
- even then, protected processes remain off-limits
- for normal development, Xcode relies on target binaries having `get-task-allow`

Practical consequence:

- profiling Studio-launched debug/dev builds is realistic
- attaching to arbitrary third-party or system processes is not a good v1 target
- "profile current run" is much easier than "profile anything on the machine"

For Studio specifically, this is good news: the hub already owns launched child processes through `RunningChild` in [studio/hub/src/build_manager.rs](/Users/admin/makepad/makepad/studio/hub/src/build_manager.rs:154), so "profile current run" can be a first-class action.

### 2. Unwinding remote stacks

Getting register state is easy. Turning that into correct call stacks is the real engineering work.

For a native sampler, the clean MVP is:

- require frame pointers in profiled builds
- unwind by walking frame chains with remote memory reads
- support `arm64/arm64e` and `x86_64`

This is very different from a fully general unwinder.

Without frame pointers, you need reliable DWARF unwind interpretation against a remote stopped thread. That is much more work and much easier to get wrong.

### 3. Apple silicon / arm64e details

The SDK headers are a warning sign here. `arm_thread_state64_t` pointer fields may be opaque on arm64e and use accessor macros. The headers also include pointer-auth stripping helpers in `mach/arm/_structs.h`.

Practical consequence:

- do not treat Apple-silicon thread state as a dumb POD register dump forever
- use the platform accessors/macros or equivalent logic
- expect extra work around `pc`, `lr`, `sp`, and `fp`

This is one reason a frame-pointer-only MVP should still be treated as non-trivial.

### 4. Symbolication

Sampling only gives you addresses. A usable flamegraph wants function names, and ideally file/line.

There are two reasonable routes:

1. Use Apple's symbolication tooling for early versions.
2. Do symbolication in Rust.

Apple's `atos(1)` can symbolicate addresses either against a live process (`-p`) or against a binary/dSYM plus load address (`-o` + `-l`). That makes it useful as a reference tool and possibly as an early fallback.

If we keep it inside Rust, the most promising path is:

- image map from `TASK_DYLD_INFO` / `dyld_all_image_infos`
- Mach-O parsing via `object`
- DWARF lookup via `addr2line`

The current `addr2line` docs are encouraging here: its `Loader` supports Mach-O dSYM files and split DWARF files.

### 5. Output shape for flamegraphs

Flamegraphs want folded stacks.

The useful Rust path is:

- aggregate stacks into folded lines like `main;foo;bar 42`
- feed that to `inferno`
- write SVG

`inferno::flamegraph::from_lines` explicitly accepts folded lines and produces SVG. It also supports differential flamegraphs if we want before/after comparisons later.

## Recommendation

Do not build v1 as a parser for `sample` output.

Why:

- `sample` produces a condensed textual call tree, not a stable raw stack API
- parsing its report format is brittle
- it is useful as a validation oracle, but not as our foundation

Do not make `xctrace` the foundation either.

Why:

- it is tied to Xcode/Instruments
- it records `.trace` files rather than a profiler-native data model
- export paths are better for interoperability than for owning the feature

Build a native Rust sampler and use Apple tools only for validation and fallback symbolication.

## Suggested architecture

### Split it into two layers

1. `libs/sampling_profiler`
   - sampling loop
   - remote unwind
   - image mapping
   - folded stack aggregation
   - optional symbolication helpers

2. `studio/profiler-cli`
   - thin command-line wrapper
   - attach/launch arguments
   - output files
   - JSON metadata for Studio

Then Studio hub drives the CLI.

That keeps the collector usable from:

- Makepad Studio
- AI tools
- shell scripts
- regression tests

### Why the hub should own it

The hub is already the correct authority for:

- current build/run selection
- child process ownership
- starting/stopping helper processes
- shipping results to the desktop UI

The desktop should not be the process/PID authority.

## MVP scope I would choose

### V1 target

- macOS only
- profile Studio-launched build only
- launch from Studio hub
- fixed-duration capture
- fixed sample rate
- frame-pointer unwind only
- function-name symbolication only at first
- folded stack output
- SVG flamegraph export

### Explicit non-goals for V1

- arbitrary attach to protected processes
- perfect DWARF unwinding without frame pointers
- off-CPU flamegraphs
- memory flamegraphs
- native in-Studio SVG renderer if that slows delivery too much

## Rust build requirements for profiled targets

For Rust code, require:

- `-C force-frame-pointers=yes`
- `-C debuginfo=1` or `2`

The Rust codegen docs also note that macOS defaults to packed split debuginfo, which means dSYM output is already the normal shape there.

This is important because the easiest native MVP assumes:

- stable frame chains
- symbol data available somewhere predictable

If we do not require frame pointers, the implementation cost jumps immediately.

## Concrete collector design

### Sampling loop

For each capture:

1. Resolve target PID from the current Studio run.
2. Get task port with `task_for_pid`.
3. Snapshot loaded image metadata with `TASK_DYLD_INFO` and `dyld_all_image_infos`.
4. Loop for `duration * hz` iterations:
   - enumerate threads with `task_threads`
   - suspend task or suspend threads
   - read thread registers with `thread_get_state`
   - unwind each thread using frame pointers plus `mach_vm_read_overwrite`
   - resume task or threads
   - aggregate stack into a hash map keyed by normalized frame sequence
5. Symbolicate addresses.
6. Write:
   - `capture.folded`
   - `capture.svg`
   - `capture.json`

### Unwind strategy

For MVP:

- arm64: seed with `pc`, then walk `fp` chain
- x86_64: seed with `rip`, then walk `rbp` chain
- validate every remote read
- stop on bad pointers, loops, non-monotonic frames, or depth cap

This will work well enough for Studio dev builds if we control the frame-pointer story.

### Symbolication strategy

I would do this in two passes:

1. v1
   - image UUID/load-address map
   - symbol names only
   - fall back to raw hex addresses if needed

2. v2
   - dSYM lookup by UUID
   - file/line
   - inline frames

`atos` is useful as a correctness oracle during development, even if the product path stays in Rust.

## Studio integration plan

### Do not overload the current profiler protocol

Instead, add a separate sampling-profile path. For example:

- hub command: `StartSamplingProfile { build_id, duration_ms, hz }`
- hub message: `SamplingProfileProgress`
- hub message: `SamplingProfileFinished { folded_path, svg_path, metadata_path }`

### UI shape

There are two reasonable options:

1. Ship fast
   - Studio launches capture
   - Studio shows status/progress
   - Studio opens the generated SVG artifact

2. Better long-term
   - Studio stores folded stacks or a prebuilt frame tree
   - new native `FlamegraphPane` renders directly in Makepad
   - SVG remains an export/share format, not the primary UI

If "lives inside Studio" is a hard requirement, I would still keep SVG export, but I would not make SVG parsing/rendering the internal source of truth.

The best long-term model is:

- folded stacks / tree as source data
- native flamegraph widget in Studio
- SVG as export

## Estimated effort by area

### Native MVP

- hub plumbing and command wiring: `1-2 days`
- macOS task/thread sampling loop: `2-3 days`
- frame-pointer unwind on arm64 and x86_64: `3-5 days`
- symbolication MVP: `2-4 days`
- folded output + `inferno` SVG: `1 day`
- Studio result tab / artifact flow: `2-4 days`
- tests, failure handling, retries: `2-4 days`

Total: roughly `1-2 weeks` if kept tight.

### Product-quality version

Add all of the following and the timeline moves materially:

- arbitrary attach UX
- better permission story
- dSYM lookup by UUID
- inline frames and file/line
- arm64e hardening
- cancellation and long-running captures
- differential flamegraphs
- persistent history
- native interactive flamegraph widget

Total: roughly `4-8 weeks`.

## What I would build first

### Phase 1

- native Rust sampler
- Studio-launched build only
- frame-pointer-only unwind
- folded output
- SVG via `inferno`
- command-line entrypoint
- Studio button: "Sample CPU"

### Phase 2

- better symbolication
- native flamegraph pane
- diff captures
- filters by thread / image / symbol

### Phase 3

- optional attach to existing PID
- off-CPU or wait-state profiling
- profile import/export

## Bottom line

This is a good feature to build, but only if we are strict about scope.

If the requirement is:

- real out-of-process sampling
- flamegraph output
- works for the app Studio launched

then Rust is a good fit and the MVP is very doable.

If the requirement expands to:

- arbitrary macOS process attach
- no frame-pointer assumptions
- product-grade symbolication
- native Studio flamegraph UX on day one

then it becomes a much bigger project.

My recommendation is:

- build a native Rust sampler
- target Studio-launched debug builds first
- require frame pointers for v1
- use folded stacks as the core format
- export SVG with `inferno`
- keep the current Studio profiler separate from this feature

## Sources

- Apple `sample(1)` man page: `man sample`
- Apple `taskgated(8)` man page: `man taskgated`
- Apple `atos(1)` man page: `man atos`
- Apple SDK headers:
  - `mach/mach_traps.h`
  - `mach/task.h`
  - `mach/thread_act.h`
  - `mach/mach_vm.h`
  - `mach/task_info.h`
  - `mach-o/dyld_images.h`
  - `mach/arm/_structs.h`
- Apple docs:
  - https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.cs.debugger
  - https://developer.apple.com/documentation/kernel/1537751-task_threads/
  - https://developer.apple.com/documentation/kernel/1418576-thread_get_state
  - https://developer.apple.com/documentation/kernel/mach/vm
  - https://developer.apple.com/documentation/xcode/xcode-command-line-tool-reference
- Rust docs:
  - https://doc.rust-lang.org/stable/rustc/codegen-options/
  - https://docs.rs/inferno/latest/inferno/flamegraph/
  - https://docs.rs/inferno/latest/inferno/flamegraph/fn.from_lines.html
  - https://docs.rs/addr2line/
  - https://docs.rs/mach2/latest/mach2/task/
  - https://docs.rs/mach2/latest/mach2/thread_act/
  - https://docs.rs/mach2/latest/mach2/vm/
- Flamegraph reference:
  - https://www.brendangregg.com/flamegraphs.html
