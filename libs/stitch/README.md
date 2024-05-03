# Stitch

Stitch is an experimental Webassembly interpreter written in Rust that is designed to be very
lightweight and fast.

## Caution

Stitch achieves its speed by relying on sibling call optimisation (sibling calls are a restricted
form of tail calls in which the callee has the same signature as the caller).  Rust doesn’t provide
a mechanism to guarantee that tail calls are optimised, but in practice, LLVM always automatically
optimises sibling calls on 64-bit platforms, provided certain constraints are met.

Note that LLVM does not strictly guarantee to optimise sibling calls either, so in theory it is
possible for this feature to regress, which would cause Stitch to stop working. In practice, this
is very unlikely, as such a regression would have a significant negative impact on the performance
of existing codebases, but the possibility cannot be ruled out entirely.

If you’re considering to use Stitch for your project, please be aware of the above risk.

## Compile time

Stitch is very lightweight. It has 0 non-dev dependencies, and consists of roughly 15.000 lines of
code. On my MacBook Pro M2, it compiles from scratch in 3.37s. This is several times better than
Wasmi (16.79s), which is the only other WebAssembly interpreter written in Rust that I’m aware of.

## Performance

Stitch is very fast. On my MacBook Pro M2, it achieves a Coremark score of 3016, which is several
times better than Wasmi (785), and slightly better than Wasm3 (2907), which is the fastest
WebAssembly interpreter that I’m aware of.

## Features

Stitch supports all features in Release 2.0 (Draft 2024-04-28) of the WebAssembly Specificiation,
with the exception of SIMD.

Stitch notably does not yet support Wasi or gas metering. There’s no reason it couldn’t support
either, I just haven’t had the time to get around to it :-)

## Portability

Stitch compiles and passes the WebAssembly core test suite on 64-bit Windows, Mac, and Linux.

Stitch currently does not run on 32-bit platforms. The reason is that I’ve been unable to get LLVM
to perform sibling call optimisation on these platforms (if anyone has any ideas on this, I’d very
much love to hear them!).

We could conceivably create a fallback mode that relies on a trampoline instead of tail recursion
for these platforms, but it would largely negate the speed benefits of Stitch. Either way, I
haven’t yet had the time to look into it :-)

If both speed and broad portability are a concern, Wasm3 might be a better choice for you.

## Security

Stitch has a safe API, but uses a significant amount of unsafe code under the hood. This code has
not been audited in any way, so I cannot give any strong guarantees with respect to safety. If
safety is a concern, Wasmi might be a better choice for you.

That said, I’ve made a serious effort to ensure that all the unsafe code in Stitch is sound, and
conforms to the stacked borrows model. Unfortunately, it’s not possible to run Stitch in Miri at
the moment, since Miri doesn’t optimise sibling calls.Stitch

Stitch is an experimental Webassembly interpreter written in Rust that is designed to be very
lightweight and fast.

## Caution

Stitch achieves its speed by relying on sibling call optimisation (sibling calls are a restricted
form of tail calls in which the callee has the same signature as the caller). Rust doesn’t provide
a mechanism to guarantee that tail calls are optimised, but in practice, LLVM always automatically
optimises sibling calls on 64-bit platforms, provided certain constraints are met.

Note that LLVM does not strictly guarantee to optimise sibling calls either, so in theory it is
possible for this feature to regress, which would cause Stitch to stop working. In practice, this
is very unlikely, as such a regression would have a significant negative impact on the performance
of existing codebases, but the possibility cannot be ruled out entirely.

If you’re considering to use Stitch for your project, please be aware of the above risk.

## Compile time

Stitch is very lightweight. It has 0 non-dev dependencies, and consists of roughly 15.000 lines of
code. On my MacBook Pro M2, it compiles from scratch in 3.37s. This is several times better than
Wasmi (16.79s), which is the only other WebAssembly interpreter written in Rust that I’m aware of.

## Performance

Stitch is very fast. On my MacBook Pro M2, it achieves a Coremark score of 3016, which is several
times better than Wasmi (785), and slightly better than Wasm3 (2907), which is the fastest
WebAssembly interpreter that I’m aware of.

## Features

Stitch supports all features in Release 2.0 (Draft 2024-04-28) of the WebAssembly Specificiation,
with the exception of SIMD.

Stitch notably does not yet support Wasi or gas metering. There’s no reason it couldn’t support
either, I just haven’t had the time to get around to it.

## Portability

Stitch compiles and passes the WebAssembly core test suite on 64-bit Windows, Mac, and Linux.

Stitch currently does not run on 32-bit platforms. The reason is that I’ve been unable to get LLVM
to perform sibling call optimisation on these platforms (if anyone has any ideas on this, I’d very
much love to hear them!).

We could conceivably create a fallback mode that uses a trampoline instead of tail recursion for
these platforms, but it would largely negate the speed benefits of Stitch. Either way, I haven’t yet
had the time to look into this.

If both speed and broad portability are a concern, Wasm3 might be a better choice for you.

## Safety

Stitch has a safe API, but uses a significant amount of unsafe code under the hood. This code has
not been audited in any way, so I cannot give any strong guarantees with respect to safety. If
safety is a concern, Wasmi might be a better choice for you.

That said, I’ve made a serious effort to ensure that all the unsafe code in Stitch is sound, and
conforms to the stacked borrows model. Unfortunately, it’s not possible to run Stitch in Miri at
the moment, since Miri doesn’t optimise sibling calls.

## Usage

### As CLI application

To install the latest version of Stitch:

    cargo install makepad-stitch

To run a Webassembly binary:

    makepad-stitch <wasm_file> <func_name> [<args>*]