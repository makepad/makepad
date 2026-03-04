[Top level](../README.md)

# System Requirements

## Operating System

The SVT-AV1 Encoder has been validated to run on Windows and Linux 64-bit
operating systems and the list below represents the specific OS configurations
that the encoder application and library were tested and validated on:

- __Windows (64-bit):__
  - Windows Server 2016
- __Linux (64-bit):__
  - Ubuntu 16.04 Server LTS
  - Ubuntu 18.04 Server LTS
  - Ubuntu 20.04 Server LTS
- __Other Unix-like (64-bit):__
  - MacOS

## Hardware

The SVT-AV1 Encoder library primarily supports the x86 architecture with handwritten
simd assembly code, however, the encoder can be compiled and run on any architecture
that a valid C99 compiler can target, with varying limited support for non-x86 CPUs.

- __CPU Requirements__

  In order to achieve the performance targeted by the SVT-AV1 Encoder, a modern
  x86-64 CPU is highly recommended with at least AVX2 support when running the
  encoder, however, simd support is extended all the way back to sse2.

- __RAM Requirements__

  The SVT-AV1 Encoder will adapt itself to the system on which it is being run,
  albeit only to a slight degree. The memory requirements for encoding will
  depend primarily on the number of cores the encoder will target (`--lp`),
  the input resolution and bit-depth (`-h`, `-w`, and `--input-depth`), look
  ahead distance (`--lookahead`), and hierarchical levels (`--hierarchical-levels`).
  The encoder will display an error if the system lacks enough RAM prior to the
  start of the encode.
