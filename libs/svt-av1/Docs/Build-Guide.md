[Top level](../README.md)

# Build and Install
Before building, please consider **enabling** link-time optimizations (LTO), which will build a binary with reduced call and return overhead due to the increased inline expansion. In order for effective LTO generated inline expansion to occur, additional time to process the source files is needed, and so may affect your overall compile time.

## Windows* Operating Systems (64-bit)

- __Build Requirements__
  - Visual Studio* 2017 (download [here](https://www.visualstudio.com/vs/older-downloads/)) or 2019 (download [here](https://visualstudio.microsoft.com/downloads/))
  - CMake 3.16 or later (download [here](https://github.com/Kitware/CMake/releases/download/v3.24.2/cmake-3.24.2-windows-x86_64.msi))
  - NASM Assembler version 2.14 or later
    - Download the nasm exe from the following [link](https://www.nasm.us/pub/nasm/releasebuilds/2.16.03/win64/nasm-2.16.03-win64.zip)
    - Extract the archive
    - Copy nasm.exe into a location that is in the `PATH` environment variable

- __Build Instructions__
  - Build the project by following the steps below
    - cd into `Build\windows`
    - run `build.bat <2019|2017|2015>` [This will generate the .sln files and build the project]
    - for LTO build run `build.bat <2019|2017|2015> lto` [This will generate the .sln files and build the project]

- __Binaries Location__
  - Binaries can be found under `<repo dir>/Bin/Release` or `<repo dir>/Bin/Debug`, depending on whether Debug or Release were selected in the build mode.

- __Installation__

  For the binaries to operate properly on your system, the following conditions have to be met:
  - On any of the Windows* Operating Systems listed in the OS requirements section, install Visual Studio* 2015/2017/2019
  - Once the installation is complete, copy the binaries to a location making sure that both the sample application `SvtAv1EncApp.exe` and library `SvtAv1Enc.dll` are in the same folder.
  - Open the command prompt window at the chosen location and run the sample application to encode: `SvtAV1EncApp.exe -i [in.yuv] -w [width] -h [height] -b [out.ivf]`
  - Sample application supports reading from pipe. E.g. `ffmpeg -i [input.mp4] -nostdin -f rawvideo -pix_fmt yuv420p - | SvtAv1EncApp.exe -i stdin -n [number_of_frames_to_encode] -w [width] -h [height]`

## Linux* Operating Systems (64-bit)

Note - a Dockerfile is provided to build the encoder into a tiny Alpine Linux Docker image.

- __Build Requirements__
  - GCC 5.4.0 or later
  - CMake 3.16 or later
  - NASM Assembler version 2.14 or later

- __Build Instructions__
  - `cd Build/linux`
  - `./build.sh <release | debug>`
  - for an LTO build, run `./build.sh <release | debug> --enable-lto`
  - for a PGO build, run `./build.sh <release | debug> --enable-pgo`
    (LTO and PGO can be combined, for maximum gains)

- __Sample Binaries location__
  - Binaries can be found under `Bin/Release` and/or `Bin/Debug`

- __Clang usage__
  - To install Clang-11 on Ubuntu 20.04 execute single command: `sudo apt install clang-11`
  - To install Clang-11 on Ubuntu 18.04 execute commands:
    - `wget -O - https://apt.llvm.org/llvm-snapshot.gpg.key | sudo apt-key add -`
    - `sudo apt-add-repository "deb http://apt.llvm.org/bionic/ llvm-toolchain-bionic-11 main"`
    - `sudo apt install clang-11`
  - To build SVT-AV1 using Clang-11:
    - `export CC="clang-11"`
    - `export CXX="clang++-11"`
    - `./build.sh <release | debug>`


- __Installation__

  For the binaries to operate properly on your system, the following conditions have to be met:

  - On any of the Linux* Operating Systems listed above, copy the binaries under a location of your choice.
  - Change the permissions on the sample application `SvtAV1EncApp` executable by running the command: `chmod +x SvtAv1EncApp`
  - cd into your chosen location
  - Run the sample application to encode: `./SvtAv1EncApp -i [in.yuv] -w [width] -h [height] -b [out.ivf]`
  - Sample application supports reading from pipe. E.g. `ffmpeg -i [in.mp4] -map 0:v:0 -pix_fmt yuv420p -f yuv4mpegpipe -strict -1 - | ./SvtAv1EncApp -i stdin -n [number_of_frames_to_encode] -b [out.ivf]`

# SVT-AV1 ffmpeg plugin installation

## 1. Build and install SVT-AV1

``` bash
git clone --depth=1 https://gitlab.com/AOMediaCodec/SVT-AV1.git
cd SVT-AV1
cd Build
cmake .. -G"Unix Makefiles" -DCMAKE_BUILD_TYPE=Release
make -j $(nproc)
sudo make install
```

## 2. Enable libsvtav1 in FFmpeg

NOTE: If you wish to use an FFmpeg tag or release before 4.4, please go
[here](https://gitlab.com/AOMediaCodec/SVT-AV1/tree/v0.8.4/ffmpeg_plugin) and
consult that page to properly patch ffmpeg for use with SVT-AV1.

``` bash
   git clone --depth=1 https://github.com/FFmpeg/FFmpeg ffmpeg
   cd ffmpeg
   export LD_LIBRARY_PATH+=":/usr/local/lib"
   export PKG_CONFIG_PATH+=":/usr/local/lib/pkgconfig"
   ./configure --enable-libsvtav1
   make -j $(nproc)
```

## 3. Verify that ffmpeg is working

``` bash
./ffmpeg -i input.mp4 -c:v libsvtav1 -y test.mp4
```

# How to evaluate by ready-to-run executables with docker

Refer to the guide [here](https://github.com/OpenVisualCloud/Dockerfiles/blob/master/doc/svt.md#Evaluate-SVT).

# Demo features and limitations

- **Multi-instance support:** The multi-instance functionality is a demo
  feature implemented in the SVT-AV1 Encoder sample application as an example
  of one sample application using multiple encoding libraries. Encoding using
  the multi-instance support is limited to only 6 simultaneous streams. For
  example two channels encoding on Windows: `SvtAV1EncApp.exe -nch 2 -c
  firstchannel.cfg secondchannel.cfg`
- **Features enabled:** The library will display an error message any
  feature combination that is not currently supported.


