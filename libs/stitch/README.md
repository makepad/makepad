# Stitch

Stitch is an experimental Wasm interpreter written in Rust that is designed to be very fast and lightweight.

## Caution

If you're planning to use Stitch for your project, you should consider the following carefully:

Stitch achieves its speed by relying on sibling call optimisation (sibling calls are a restricted form of tail calls in which the callee has the same signature as the caller).  Rust doesn’t provide a mechanism to guarantee that tail calls are optimised, but in practice, LLVM automatically optimises sibling calls on 64-bit platforms, provided certain constraints are met.

Note that LLVM does not strictly guarantee to optimise sibling calls either, so in theory it is possible for this feature to regress, which would cause Stitch to stop working. In practice, I consider this unlikely, as such a regression would have a significant negative impact on the performance of existing code. However, the possibility cannot be ruled out entirely.

We could add a fallback mode to Stitch that relies on a trampoline instead of tail calls. This would allow Stitch to continue working in the face of LLVM regressions, but would also negate most of its speed benefits, and in any case this fallback mode has not been implemented yet.

## Performance

Stitch is very fast. I've compared it against several other engines, shown in the table here below:

| Name      | Description                                                  |
| --------- | ------------------------------------------------------------ |
| [Wasmi]   | The only other Wasm interpreter written in Rust that I know. |
| [Wasm3]   | The fastest Wasm interpreter written in C that I know.       |
| [Wasmtime] | A JIT compiler for Wasm.                                    |

[Wasmi]: https://github.com/wasmi-labs/wasmi
[Wasm3]: https://github.com/wasm3/wasm3
[Wasmtime]: https://github.com/bytecodealliance/wasmtime

### Coremark Results

The following table shows the Coremark scores for Stitch and the other engines I've compared it against on the three major 64-bit platforms:

| Engine   | Mac   | Linux | Windows |
| -------- | ----- | ----- | ------- |
| Stitch   | 2950  | 1684  | 4592    |
| Wasm3    | 2911  | 1734  | 3951    |
| Wasmi    | 788   | 645   | 1574    |
| Wasmtime | 12807 | 13724 | 34796   |

The following table shows the CPU I've used for each platform:

| Platform | CPU                        |
| -------- | -------------------------- |
| Mac      | Apple M2 Pro @ 3.5GHz      |
| Linux    | Intel Xeon E312xx @ 2.1GHz |
| Windows  | Intel i9-13900K @ 3.0GHz   |


As you can see, Stitch is several times faster than Wasmi on all major platforms. Compared to Wasm3, it is slightly faster on Mac, slightly slower on Linux, and significantly faster on Windows.

Stitch is much slower than Wasmtime (about 4x on Mac, about 8x on Linux and Windows), but that is to be expected, given that Wasmtime is a JIT compiler, whereas Stitch is an interpreter.

The reason Stitch is faster than Wasmi is that Stitch uses threaded code, whereas Wasmi uses an interpreter loop.

The reason Stitch is slightly faster than Wasm3 on Mac, but slightly slower on Linux, is likely because Stitch has more variants per instruction compared to Wasm3, which puts pressure on the instruction cache. I suspect this gives Stitch the edge on the Apple M2 Pro, with its large instruction cache, but Wasm3 the edge on the Intel Xeon E312xx, with its smaller instruciton cache.

The reason Stitch is significantly faster than Wasm3 on Windows is likely because Stitch uses the System V calling convention for its threaded code, while Wasm3 uses the vectorcall calling convention, which has fewer registers available to pass integer arguments.

The above results were obtained by going to the `coremark` directory and running:

    cargo run --release

This will run the `coremark-minimal.wasm` testsuite and print the results for each of the engines above.

### Micro Benchmarks

To get a better idea of how fast Stitch is, I've also ran it against several micro benchmarks. The following table shows the results for each of these benchmarks on Mac (using an Apple M2 Pro @ 3.5GHz):

| Name               | Time      |
| ------------------ | --------- |
| fac_iter 1_048_576 | 2.8561 ms |
| fac_rec  32        | 276.82 ns |
| fib_iter 1_048_576 | 5.0592 ms |
| fib_rec  32        | 52.136 ms |
| fill 0 1_048_576   | 3.9256 ms |
| sum 0 1_048_576    | 4.4489 ms |

The above results were obtained by running:

    cargo bench

## Compile time

Stitch is very lightweight. It has 0 non-dev dependencies, and consists of roughly 15.000 lines of code.

The following table shows how fast Stitch compiles on Mac compared to other Wasm engines written in Rust (using an Apple M2 Pro @ 3.5GHz):

| Engine   | Compile Time |
| -------- | ------------ |
| Stitch   | 2.29s        |
| Wasmi    | 15.61s       |
| Wasmtime | 81.02s       |

As you can see, Stitch compiles much faster than either Wasmi or Wasmtime. If compile time is important to you, Stitch might be a good choice for you.

## Features

Stitch supports the following finished Wasm proposals:

| Proposal                                           | Status |
| -------------------------------------------------- | ------ |
| [Import/Export of Mutable Globals]                 | ✅     |
| [Non-trapping float-to-int conversions]            | ✅     |
| [Sign-extension operators]                         | ✅     |
| [Multi-value]                                      | ✅     |
| [JavaScript BigInt to WebAssembly i64 integration] | ❌     |
| [Reference Types]                                  | ✅     |
| [Bulk memory operations]                           | ✅     |
| [Fixed-width SIMD]                                 | ❌     |
|                                                    |       |
| [WASI]                                             | ❌     |

[Import/Export of Mutable Globals]: https://github.com/WebAssembly/mutable-global
[Non-trapping float-to-int conversions]: https://github.com/WebAssembly/nontrapping-float-to-int-conversions
[Sign-extension operators]: https://github.com/WebAssembly/sign-extension-ops
[Multi-value]: https://github.com/WebAssembly/multi-value
[JavaScript BigInt to WebAssembly i64 integration]: https://github.com/WebAssembly/JS-BigInt-integration
[Reference Types]: https://github.com/WebAssembly/reference-types
[Bulk memory operations]: https://github.com/WebAssembly/bulk-memory-operations
[Fixed-width SIMD]: https://github.com/webassembly/simd
[WASI]: https://github.com/WebAssembly/WASI

## Portability

Stitch compiles and passes the Wasm core test suite on all the three major 64-bit platforms (Mac, Linux, and Windows).

Stitch currently does not run on 32-bit platforms. The reason for this is that I have not yet found a way to get LLVM to perform sibling call optimisation on these platforms (ideas welcome).

If you need broader portability than this, either Wasmi or Wasm3 might be a better choice for you.

## Security

Stitch has a safe API, but uses a significant amount of unsafe code under the hood. This unsafe code has not been audited in any way, so I cannot give any strong guarantees about safety. If you need strong guarantees about safety, Wasmi might be a better choice for you.

That said, I’ve made a serious effort to ensure that all unsafe code in Stitch is sound, and conforms to the stacked borrows model. Unfortunately, it's currently not possible to run Stitch in Miri, since Miri is an interpreter, and therefore doesn't optimize sibling calls.

## Usage

### As a CLI Application 

To install the CLI:

    cargo install makepad-stitch

To run a Wasm binary:

    makepad-stitch <file_name> <func_name> [<arg>]*

### As a Rust Library

To learn how to use Stitch as a Rust library, please refer to the Stitch crate docs.

## Design

TODO
