[Top level](../README.md)

# Build and Install for aarch64 targets

## Builds using cross compilers

- __Build Requirements__
  - CMake 3.23 or later
  - Toolchain (download [here](https://snapshots.linaro.org/gnu-toolchain/13.0-2022.09-1/aarch64-linux-gnu/gcc-linaro-13.0.0-2022.09-x86_64_aarch64-linux-gnu.tar.xz))
  - The environment variable `PATH` ought to include the folder in the toolchain that contains `aarch64-linux-gnu-gcc`.

- __Build Instructions__
  - Run the following commands
    - Release Builds

      ```bash
      cmake -B Bin/Release -S . -DCMAKE_BUILD_TYPE="Release" -DCMAKE_TOOLCHAIN_FILE="./cmake/toolchains/aarch64_toolchain.cmake"
      cmake --build Bin/Release -j
      ```

    - Debug Builds

      ```bash
      cmake -B Bin/Debug -S . -DCMAKE_BUILD_TYPE="Debug" -DCMAKE_TOOLCHAIN_FILE="./cmake/toolchains/aarch64_toolchain.cmake"
      cmake --build Bin/Debug -j
      ```

## Native builds on an aarch64 Linux machine

- __Build Requirements__
  - GCC 5.4.0 or later
  - CMake 3.23 or later
  - YASM Assembler version 1.2.0 or later

- __Build Instructions__
  - Run the following commands
    - Release Builds

      ```bash
      cmake -B Bin/Release -S . -DCMAKE_BUILD_TYPE="Release"
      cmake --build Bin/Release -j
      ```

    - Debug Builds

      ```bash
      cmake -B Bin/Debug -S . -DCMAKE_BUILD_TYPE="Debug"
      cmake --build Bin/Debug -j
      ```

## Binaries Location

- Binaries can be found under `<repo dir>/Bin/Release` or `<repo dir>/Bin/Debug`, depending on whether Debug or Release were selected in the build mode.

## Installation

  For the binaries to operate properly on your system, the following conditions have to be met:

- On any of the Linux* Operating Systems listed above, copy the binaries under a location of your choice.
- Change the permissions on the sample application `SvtAV1EncApp` executable by running the command: `chmod +x SvtAv1EncApp`
- cd into your chosen location
- Run the sample application to encode

   ```bash
   ./SvtAv1EncApp -i [in.yuv] -w [width] -h [height] -b [out.ivf]
   ```

- Sample application supports reading from pipe. For example

   ```bash
   ffmpeg -i [input.mp4] -nostdin -f rawvideo -pix_fmt yuv420p - | ./SvtAv1EncApp -i stdin -n [number_of_frames_to_encode] -w [width] -h [height]
   ```
