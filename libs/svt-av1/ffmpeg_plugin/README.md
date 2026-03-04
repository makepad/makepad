# FFmpeg patches for SVT-AV1

This directory contains patches and backported commits that might be of interest
to users of libsvtav1 using FFmpeg tags.

For the original commits the patch was generated from, see <https://gitlab.com/1480c1/FFmpeg/> and look at the svt-av1/n* branches.

## Changes

Notes:

- The patches for n4.4 have been tested to apply cleanly to n4.4.1 and n4.4.2.
- The patches for n5.0 have been tested to apply cleanly to n5.0.1.
- The patches for n5.1 have been tested to apply cleanly to n5.1, n5.1.1, n5.1.2, and n5.1.3.
  - n5.1 already contains certain patches from the set of n5.0, thus is a subset of n5.0's patches.
- The patches for n5.1.6 have been tested to apply cleanly to n5.1.4, n5.1.5, and n5.1.6.
  - This is simply a subset of patches from n5.1, but without the patches that are already in n5.1.6.

### n6.1 - [svt-av1/n6.1](https://gitlab.com/1480c1/FFmpeg/-/tree/svt-av1/n6.1)

#### using SVT-AV1 v1.1.0 or lower with n6.1?

- [67a2571a55](https://gitlab.com/1480c1/FFmpeg/-/commit/67a2571a55): Backport patch for adding version guard around `force_key_frames`, not needed if using the latest SVT-AV1

#### Using SVT-AV1 v2.0.0 or higher with n6.1?

- [69dd1ce610](https://gitlab.com/1480c1/FFmpeg/-/commit/69dd1ce610): Backport patch for SVT-AV1 2.0.0 API changes surounding low delay operations

### n6.0 - [svt-av1/n6.0](https://gitlab.com/1480c1/FFmpeg/-/tree/svt-av1/n6.0)

#### Using SVT-AV1 v1.4 with n6.0?

- [fba9d9609f](https://gitlab.com/1480c1/FFmpeg/-/commit/fba9d9609f): Backport patch for additional changes for max_buf_sz
- [7958325a83](https://gitlab.com/1480c1/FFmpeg/-/commit/7958325a83): Backport patch for allowing -1 for the -preset option
- [7bcc1b4eb8](https://gitlab.com/1480c1/FFmpeg/-/commit/7bcc1b4eb8): Backport patch for workaround with `gop_size` == 1
- [e8465aa5dd](https://gitlab.com/1480c1/FFmpeg/-/commit/e8465aa5dd): Backport patch for for using the workaround with `force_key_frames` if `gop_size` == 1
- [67a2571a55](https://gitlab.com/1480c1/FFmpeg/-/commit/67a2571a55): Backport patch for adding version guard around `force_key_frames`
- [69dd1ce610](https://gitlab.com/1480c1/FFmpeg/-/commit/69dd1ce610): Backport patch for SVT-AV1 2.0.0 API changes surounding low delay operations

---

### n5.1.6 - [svt-av1/n5.1.6](https://gitlab.com/1480c1/FFmpeg/-/tree/svt-av1/n5.1.6)

Please refer to the n5.1 section for information about these patches.

### n5.0 - [svt-av1/n5.0](https://gitlab.com/1480c1/FFmpeg/-/tree/svt-av1/n5.0)

#### Using SVT-AV1 v1.0 with n5.0?

- [c33b404885](https://gitlab.com/1480c1/FFmpeg/-/commit/c33b404885): Backport `-svtav1-params:v`
- [1dddb930aa](https://gitlab.com/1480c1/FFmpeg/-/commit/1dddb930aa): Backport `-crf:v`, remove `-rc:v` and instead use `-b:v`, `-maxrate:v`, `-crf:v`, and `-qp:v` to set rc mode
- [50bc872635](https://gitlab.com/1480c1/FFmpeg/-/commit/50bc872635): Backport patch for using aq-mode to determine crf or qp
- [51c0b9e829](https://gitlab.com/1480c1/FFmpeg/-/commit/51c0b9e829): Backport patch for passing color description info
- [e3c4442b24](https://gitlab.com/1480c1/FFmpeg/-/commit/e3c4442b24): Backport patch for parsing svtav1-params last
- [ded0334d21](https://gitlab.com/1480c1/FFmpeg/-/commit/ded0334d21): Backport patch for choma-sample-location
- [70887d44ff](https://gitlab.com/1480c1/FFmpeg/-/commit/70887d44ff): Backport patch for not setting tbr if it's not needed
- [fe100bc556](https://gitlab.com/1480c1/FFmpeg/-/commit/fe100bc556): Backport patch for passing bitrate properties through cpb side data
- [fb70e0611b](https://gitlab.com/1480c1/FFmpeg/-/commit/fb70e0611b): Backport patch for fixing segfault on copying metadata from the EOS signal
- [7958325a83](https://gitlab.com/1480c1/FFmpeg/-/commit/7958325a83): Backport patch for allowing -1 for the -preset option
- [7bcc1b4eb8](https://gitlab.com/1480c1/FFmpeg/-/commit/7bcc1b4eb8): Backport patch for workaround with `gop_size` == 1
- [e8465aa5dd](https://gitlab.com/1480c1/FFmpeg/-/commit/e8465aa5dd): Backport patch for for using the workaround with `force_key_frames` if `gop_size` == 1
- [67a2571a55](https://gitlab.com/1480c1/FFmpeg/-/commit/67a2571a55): Backport patch for adding version guard around `force_key_frames`
- [69dd1ce610](https://gitlab.com/1480c1/FFmpeg/-/commit/69dd1ce610): Backport patch for SVT-AV1 2.0.0 API changes surounding low delay operations

#### Using SVT-AV1 v1.1 with n5.0?

- [6fd1533057](https://gitlab.com/1480c1/FFmpeg/-/commit/6fd1533057): Backport patch for passing pict_type to libsvtav1 (for force key frame feature)

#### Using SVT-AV1 v1.4 with n5.0?

- [031f1561cd](https://gitlab.com/1480c1/FFmpeg/-/commit/031f1561cd): Backport patch removing compressed_ten_bit_format
- [1c6fd7d756](https://gitlab.com/1480c1/FFmpeg/-/commit/1c6fd7d756): Backport patch removing vbv_bufsize
- [96748ac54f](https://gitlab.com/1480c1/FFmpeg/-/commit/96748ac54f): Backport patch fixing setting of max_buf_sz
- [fba9d9609f](https://gitlab.com/1480c1/FFmpeg/-/commit/fba9d9609f): Backport patch for additional changes for max_buf_sz

There is an additional patch for libavutil/hwcontext_vulkan that can help with compilation errors on n5.0.
This patch is not necessary if you have vulkan disabled.

---

### n4.4 - [svt-av1/n4.4](https://gitlab.com/1480c1/FFmpeg/-/tree/svt-av1/n4.4)

#### Using SVT-AV1 v1.0 with n4.4?

- [04b89e8ae3](https://gitlab.com/1480c1/FFmpeg/-/commit/04b89e8ae3): Backport fix for caps_internal
- [64e2fb3f9d](https://gitlab.com/1480c1/FFmpeg/-/commit/64e2fb3f9d): Backport change for gop type
- [0463f5d6d5](https://gitlab.com/1480c1/FFmpeg/-/commit/0463f5d6d5): Backport fix for rc range
- [c5f3143090](https://gitlab.com/1480c1/FFmpeg/-/commit/c5f3143090): Backport fix CQP mode, left in to allow for patch to apply cleanly
- [c33b404885](https://gitlab.com/1480c1/FFmpeg/-/commit/c33b404885): Backport `-svtav1-params:v`
- [1dddb930aa](https://gitlab.com/1480c1/FFmpeg/-/commit/1dddb930aa): Backport `-crf:v`, remove `-rc:v` and instead use `-b:v`, `-maxrate:v`, `-crf:v`, and `-qp:v` to set rc mode
- [50bc872635](https://gitlab.com/1480c1/FFmpeg/-/commit/50bc872635): Backport patch for using aq-mode to determine crf or qp
- [51c0b9e829](https://gitlab.com/1480c1/FFmpeg/-/commit/51c0b9e829): Backport patch for passing color description info
- [e3c4442b24](https://gitlab.com/1480c1/FFmpeg/-/commit/e3c4442b24): Backport patch for parsing svtav1-params last
- [ded0334d21](https://gitlab.com/1480c1/FFmpeg/-/commit/ded0334d21): Backport patch for choma-sample-location
- [70887d44ff](https://gitlab.com/1480c1/FFmpeg/-/commit/70887d44ff): Backport patch for not setting tbr if it's not needed
- [fe100bc556](https://gitlab.com/1480c1/FFmpeg/-/commit/fe100bc556): Backport patch for passing bitrate properties through cpb side data
- [fb70e0611b](https://gitlab.com/1480c1/FFmpeg/-/commit/fb70e0611b): Backport patch for fixing segfault on copying metadata from the EOS signal
- [7958325a83](https://gitlab.com/1480c1/FFmpeg/-/commit/7958325a83): Backport patch for allowing -1 for the -preset option

#### Using SVT-AV1 v1.1 with n4.4?

- [6fd1533057](https://gitlab.com/1480c1/FFmpeg/-/commit/6fd1533057): Backport patch for passing pict_type to libsvtav1 (for force key frame feature)

#### Using SVT-AV1 v1.4 with n4.4?

- [031f1561cd](https://gitlab.com/1480c1/FFmpeg/-/commit/031f1561cd): Backport patch removing compressed_ten_bit_format
- [1c6fd7d756](https://gitlab.com/1480c1/FFmpeg/-/commit/1c6fd7d756): Backport patch removing vbv_bufsize
- [96748ac54f](https://gitlab.com/1480c1/FFmpeg/-/commit/96748ac54f): Backport patch fixing setting of max_buf_sz
- [fba9d9609f](https://gitlab.com/1480c1/FFmpeg/-/commit/fba9d9609f): Backport patch for additional changes for max_buf_sz
- [7bcc1b4eb8](https://gitlab.com/1480c1/FFmpeg/-/commit/7bcc1b4eb8): Backport patch for workaround with `gop_size` == 1
- [e8465aa5dd](https://gitlab.com/1480c1/FFmpeg/-/commit/e8465aa5dd): Backport patch for for using the workaround with `force_key_frames` if `gop_size` == 1
- [67a2571a55](https://gitlab.com/1480c1/FFmpeg/-/commit/67a2571a55): Backport patch for adding version guard around `force_key_frames`
- [69dd1ce610](https://gitlab.com/1480c1/FFmpeg/-/commit/69dd1ce610): Backport patch for SVT-AV1 2.0.0 API changes surounding low delay operations

## How to build

Assuming `$PWD` == the root of your SVT-AV1 clone and you have already built
and installed SVT-AV1 and your `PKG_CONFIG_PATH` environment variable is setup
so that `pkg-config --libs SvtAv1Enc` works properly, this may require exporting
`PKG_CONFIG_PATH` to `/usr/local/lib/pkgconfig` or where your prefix is setup

For n5.0:

```bash
git clone --branch n5.0 https://github.com/FFmpeg/FFmpeg.git
git -C FFmpeg am "$PWD/ffmpeg_plugin/n5.0"/*.patch
```

For tags n4.4*, follow the above steps but replace `n5.0` with `n4.4` or whichever tag you want to use.

```bash
mkdir -p ffmpeg-build
(
    cd ffmpeg-build
    ../FFmpeg/configure --enable-libsvtav1 # Append other options as needed
)
make -C ffmpeg-build -j$(($(nproc) + 2))
```

Adapt as needed for depending on your setup

## Sample command lines

Basic ffmpeg line

```bash
ffmpeg -y -i input.mkv -c:v libsvtav1 -crf 30 output.webm
```

FFmpeg line with crf+maxrate for capped CRF

```bash
ffmpeg -y -i input.mkv -c:v libsvtav1 -crf 30 -maxrate 6M output.webm
```

FFmpeg line with svtav1-params setting lp and asm

```bash
ffmpeg -y -i input.mkv -c:v libsvtav1 -crf 30 -svtav1-params lp=4:asm=sse4_1 output.webm
```
