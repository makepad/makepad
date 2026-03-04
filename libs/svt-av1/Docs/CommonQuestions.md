[Top level](../README.md)

# Common Questions and Topics of Interest

[[_TOC_]]

# Why build with LTO

The purpose of building with link-time optimization (LTO) enabled is to reduce the call and return overhead by inline expansion - even across different files. This means that a function's call site will be replaced with the body of the function itself. In order for this inline expansion to occur, additional compile time and memory are required.

# Why build with PGO

Profile-guided optimization (PGO) reduces the compiler's guesswork on which
parts of the code deserve more aggressive optimizations, by collecting a
runtime profile of an instrumented version of the program, while it's encoding
a video file. That profile is then used to compile an optimized version, which
is slightly more performant than the usual build using compiler heuristics.

PGO makes the build longer, by having to build the program twice and run it in
between builds. It works with GCC and Clang, and using CMake directly
requires you to specify the "RunPGO" target, instead of the default "all".

PGO and LTO can be combined.

# What Presets Do

Presets control how many efficiency features are used during the encoding
process, and the intensity with which those features are used. Lower presets
use more features and produce a more efficient file (smaller file, for a given
visual quality). However, lower presets also require more compute time during the
encode process. If a file is to be widely distributed, it can be worth it to
use very low presets, while high presets allow fast encoding, such
as for real-time applications.

Generally speaking, presets 1-3 represent extremely high efficiency, for
use when encode time is not important and quality/size of the resulting
video file is critical. Presets 4-6 are commonly used by home enthusiasts
as they represent a balance of efficiency and reasonable compute time. Presets
between 7 and 13 are used for fast and real-time encoding. One
should use the lowest preset that is tolerable.

The features enabled or changed by each preset are as follows

| **Category**                | **Feature**                                 | **0** | **1** | **2** | **3** | **4** | **5** | **6** | **7** | **8** | **9** | **10** |
| --------------------------- | ------------------------------------------  | ----  | ----  | ----  | ----  | ----  | ----  | ----  | ----  | ----  | ----  | -----  |
| Prediction structure & RC   | Hierarchical levels                         | 6L    | 6L    | 6L    | 6L    | 6L    | 6L    | 6L    | 6L    | 6L    | 5L    | 5L     |
|                             | aq-mode                                     | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | max reference frame count                   | 7     | 7     | 7     | 7     | 7     | 5     | 5     | 5     | 5     | 5     | 2      |
| Motion Estimation           | Full pel Motion Estimation                  | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Hierarchical ME                             | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | subpel                                      | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
| Block Partitioning          | sb size                                     | 128   | 128   | 128   | 128   | 128   | 128   | 128   | 64    | 64    | 64    | 64     |
|                             | min block size                              | 4     | 4     | 4     | 8     | 8     | 8     | 8     | 8     | 8     | 8     | 8      |
|                             | Non-square partitions                       | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
| AV1 mode decision features  | DC                                          | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Smooth, Smooth_V, Smooth_H                  | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Directional Angular modes                   | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Paeth                                       | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Chroma from Luma (CfL)                      | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Filter intra                                | ON    | ON    | ON    | ON    | ON    | ON    | ON    | OFF   | OFF   | OFF   | OFF    |
|                             | Intra block copy (IBC) (SC)                 | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Palette prediction (SC)                     | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Single-reference prediction                 | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Compound-reference prediction               | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Eighth-pel f(resolution, qindex)            | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Interpolation Filter Search                 | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Warped motion compensation                  | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Global motion compensation                  | ON    | ON    | ON    | ON    | ON    | OFF   | OFF   | OFF   | OFF   | OFF   | OFF    |
|                             | Motion Field Motion Vector (MFMV)           | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Overlapped Block Motion Compensation (OBMC) | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | OFF    |
|                             | Inter-Intra prediction                      | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | OFF    |
|                             | Wedge prediction                            | ON    | ON    | ON    | OFF   | OFF   | OFF   | OFF   | OFF   | OFF   | OFF   | OFF    |
|                             | Difference-weighted prediction              | ON    | ON    | ON    | OFF   | OFF   | OFF   | OFF   | OFF   | OFF   | OFF   | OFF    |
|                             | Distance-weighted prediction                | ON    | ON    | ON    | OFF   | OFF   | OFF   | OFF   | OFF   | OFF   | OFF   | OFF    |
| Transform                   | Transform type search                       | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Transform Size search                       | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
| AV1 inloop filters          | Deblocking Filter                           | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | CDEF                                        | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON     |
|                             | Restoration Filter - Wiener Filter          | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | ON    | OFF   | OFF    |
|                             | Restoration Filter - SG Filter              | ON    | ON    | ON    | ON    | OFF   | OFF   | OFF   | OFF   | OFF   | OFF   | OFF    |


# Scene Change Detection

Modern video files include key frames, which are intra-coded pictures, and
inter-frames, which store only information changed since the previously encoded
reference frames. A scene change can be a reasonable time to insert a new key
frame since the previous key frame may not have much in common with subsequent
images. Insertion of key frames at scene changes is common practice but is not required.

At present, SVT-AV1 does not insert key frames at scene changes, regardless of
the `scd` parameter. It is, therefore, advisable to use a third-party splitting
program to encode videos by chunks if key frame insertion at scene changes is
desired.

Note that not inserting a key frame at scene changes is not considered a bug
nor missing feature by the SVT-AV1 team. AV1 is sufficiently flexible that when
a scene change is detected, the encoder automatically relies less on temporally
neighboring frames, which allows it to adapt to scene changes without affecting
the GOP structure (frequency of key frames).

# GOP Size Selection

GOP stands for "Group Of Pictures." Modern video files include key frames,
which save the entire intra-coded image and begin a new GOP, and delta frames,
which store only information changed since the last key frame (through
motion-compensated-prediction and residual coding). The GOP size (governed by
the `keyint` parameter) determines how frequently a key frame is inserted. In
general, key frames are far larger, in terms of bits, than delta frames are, so
using a larger GOP can significantly improve efficiency.

However, frequent key frames can be helpful in two respects:

1. They make seek times faster. Decode software must go to the nearest
   previous key frame when a seek is requested, then it must process
   all delta frames between the key frame and the desired seek point. When key
   frames are far apart, seek performance can suffer.
2. In video-on-demand applications, key frames provide improved resilience to
   lost packets. Each key frame serves as a sort of reset for the reference
   chain used by inter frames. If the key frames are too widely spaced, missed
   packets can cause relatively long-lasting visual artifacts.

For video on demand applications, it is common to use GOP sizes of
approximately one second. For example, `keyint=24` provides a key frame once
per second for a 24 fps video. Home users frequently prefer longer GOP sizes, often
in the range of 5-10 seconds. One can specify the GOP length in seconds by appending
the `s` character. For example, `keyint=5s` would have a GOP length of 5 seconds,
regardless of the video frame rate. This usage works only with `keyint`, not ffmpeg's `-g`
parameter.

# Threading and Efficiency

SVT-AV1 is specifically designed to scale well across many logical processors.
By default, SVT-AV1 only uses multi-threading techniques that do not cause a
decrease in the resulting quality/efficiency. For example, it does not use
tile-based threading, which is known to decrease quality--SVT-AV1 supports
tiles but does not use them for parallelization by default. As a result of the
reliance on threading techniques that do not degrade quality, the
video output will be the same when using `--lp 1` as `--lp n` in the
default CRF configuration.

Anecdotally, SVT-AV1 is able to fairly efficiently use about 16 processor cores
when encoding 1080p video on a preset in the 4-6 range using the default
configuration. When using high core-count systems, SVT-AV1's ability to fully
utilize all available threads drops off and additional cores provide less
incremental encoding speed. For this reason, tools that split video into
scene-based chunks can be useful if greater parallelization is desired. As
resolution increases, threading capabilities go up as well.

Note that the highest quality presets (0-3) use features that have a lot of
dependencies and may lead to lower parallel CPU usage.

# Practical Advice on Grain Synthesis

The random pattern associated with film grain (or CCD noise) is notoriously difficult
to compress. When grain is present in the original source, significant efficiency gains
can be made by deleting film grain and replacing it with synthetic grain that
does not take up significant space in the file. When `film-grain` is enabled, SVT-AV1
denoises the image, compares the resulting image with the original, and analyzes
the nature of the grain. It then inserts synthetic grain with similar
properties. This can greatly improve efficiency while retaining visual quality and
character.

The process of removing grain can sometimes delete very fine detail from the
original image, so it should not be used too aggressively. The level passed
to the `film-grain` parameter controls how aggressively this procedure is
employed. As a general rule, a `film-grain` level of around 8 is sufficient for
live action video with a normal amount of grain. Noisier video benefits
from higher levels in the 10-15 range. 2-D animation typically has less grain,
so a level of around 4 works well with standard (hand-drawn) animation. Grainy
animation can benefit from higher levels up to around 10.

If grain synthesis levels can be manually verified through subjective evaluation or
high fidelity metrics, then passing `film-grain-denoise=0` may
result in higher fidelity by disabling source denoising. In that case, the
correct `film-grain` level is important because a more conservative smoothing
process is used--too high a `film-grain` level can lead to noise stacking.

More detail on film grain synthesis is available in the [appendix](Appendix-Film-Grain-Synthesis.md).

# Improving Decoding Performance

Although modern AV1 decoders (such as dav1d and hardware decoders) are
extremely efficient, there can be cases where software decoders on slower
hardware have a difficult time decoding AV1 streams without stuttering.

Tips that may improve decoding performance include:

* Use the `fast-decode=1` parameter
* Reduce the bitrate used (higher CRF values result in lower bitrates)
* Encode using tiles (`tile-columns=2`, for example)
* Avoid the use of synthetic grain
* Use higher presets (which do not use the more complex AV1 tools)
* Provide the option of a lower resolution version of the video
* Use 8-bit color depth instead of 10

Note that each of these options has the potential to reduce the image
quality to a greater or lesser degree, so they should be used with care.
The performance gains may also depend on the decoding platform. For example,
using tiles via the `tile-columns` and/or `tile-rows` options can lead
to large improvements in encoding and decoding performance if both the encoder
and decoder have sufficient threads available. However, if the target platform
does not support multithreaded tile decoding, then no decoding gains will be
realized. Tiling can lead to visible artifacts, especially if many
tiles are used. The use of `fast-decode=1`, on the other hand, may provide
decoding performance improvement even if the target decoder does not have
multithreading. It can also affect video quality.

Note that, for more advanced media players and streaming tool implementations,
enabling GPU AV1 grain synthesis can actually increase decoding speeds and
offset the slight decoding latency penalty.

# Tuning for Animation

There are two types of video that are called "animation": hand-drawn 2D
and 3D animated. Both types tend to be easy to encode (meaning the resulting
file will be small), but for different reasons. 2D animation often has large
areas that do not move, so the difference between one frame and another is
often small. In addition, it tends to have low levels of grain.
Experience has shown that relatively high `crf` values with low levels of
`film-grain` produce 2D animation results that are visually good.

3D animation has much more detail and movement, but it sometimes has no grain
whatsoever, or only small amounts that were purposely added to the image. If
the original animated video has no grain, encoding without `film-grain` will
increase encoding speed and avoid the possible loss of fine detail that can
sometimes result from the denoising step of the synthetic grain process.

# 8 or 10-bit Encoding

Video may be encoded with either 8 or 10-bit color depth. 10-bit video
can represent more shades of grey and colors and is less prone to certain artifacts, such as color
banding and loss of detail in low luma areas. Most SDR sources come in 8-bit
color and SVT-AV1, by default, will encode 8-bit video to 8-bit video or 10-bit
to 10-bit.

It is possible to encode 8-bit video to a 10-bit final result. This allows the
encoder to use less rounding and can produce slightly better fidelity. There is
a small cost in terms of resulting file size (~5%), and, with some encoders, an
encoding-time cost. SVT-AV1 was carefully designed to be able to encode 10-bit
color in a compute-efficient manner, so there should not be much of a
encoding performance penalty associated with 10-bit except at very fast presets
(11-13), where the slowdown may be more noticeable. One should be aware,
however, that 10-bit *decoding* can also be more compute-intensive than 8-bit
in some decoders.

To force the final result to be 10-bit, specify `-pix_fmt yuv420p10le` in ffmpeg, or you
can force 8-bit using `-pix_fmt yuv420p` in ffmpeg.

# HDR and SDR Video

Some video sources now come designed for display on high dynamic range (HDR) hardware.
SVT-AV1 can encode these sources correctly and embed the required metadata in the final
output. High definition typically involves a high definition color space (such as BT.2020)
and an associated set of transfer functions (such as PQ or HLG). HDR video is also
typically encoded with 10-bit color. Detailed information on forcing these
settings is available in the [full parameters description](parameters.md).

Colors can be mapped from a wider space into a less wide space (such as the
frequently used BT.709) using ffmpeg or other tools, but there are always
losses, either in the accuracy of the color or the retention of detail in high
and low brightness areas. As a rule, it is best to avoid conversion from HDR to
SDR, if possible.

# Options That Give the Best Encoding Bang-For-Buck

The quality/filesize tradeoff is controlled by the `crf` parameter. Increasing
this parameter can significantly reduce the file size. AV1 is very efficient at
preserving the types of details that humans notice, so significant reduction in
objective quality (PNSR and similar measures) can still result in a video with
good subjective quality.

[Film grain synthesis](#practical-advice-on-grain-synethsis) can also significantly
reduce file size while retaining apparent visual quality. It is not enabled by
default because not all sources have film grain or CCD noise, Additionally,
the de-noising process used in this procedure can delete very fine details
(such as small, fast-moving particles or skin imperfections), so the
aggressiveness of the denoising/synthesis needs to be paired with the strength
of the grain/noise in the original sample.

The use of subjective mode (`--tune=0`) often results in an image with greater
sharpness and is intended to produce a result that appears to humans
to be of high quality (as opposed to doing well on basic objective measures, such as
PSNR).

# Multi-Pass Encoding

Some encoder features benefit from or require the use of a multi-pass encoding
approach. In SVT-AV1, in general, multi-pass encoding is useful for achieving a
target bitrate when using VBR (variable bit rate) encoding, although both
one-pass and multi-pass modes are supported.

When using CRF (constant visual rate factor) mode, multi-pass encoding is
designed to improve quality for corner case videos--it is particularly helpful
in videos with high motion because it can adjust the prediction structure (to
use closer references, for example). Multi-pass encoding, therefore, can be
said to have an impact on quality in CRF mode, but is not critical in most
situations. In general, multi-pass encoding is not as important for SVT-AV1 in CRF
mode than it is for some other encoders.

CBR (constant bit rate) encoding is always one-pass.

# Bitrate Control Modes

SVT-AV1 supports three general approaches to controlling the bitrate.

* In CBR (constant bit rate) mode, the target bitrate is forced at all times.
This results in a predictable file size, but inefficient use of space as simple
scenes get too many bits and complex scenes get too few. Use `--rc=2` for this mode.
* In VBR (variable bit rate) mode, a target bitrate it set, but the effective bitrate
can vary above and below the target. Use `--rc=1` to enable VBR mode and set
the flexibility of the effective bitrate using `--bias-pct`. A value closer to
zero makes the encode behave more like a CBR encode, while a value closer to
100 gives it greater flexibility. In the VBR mode, the rate control algorithm matches the rate over the sequence.
Use `--gop-constraint-rc 1` to enable rate matching over each gop.
This feature is currently supported with VBR mode when Gop size is greater than 119.
* CRF (constant rate factor) mode targets a constant visual quality. This approach
leads to a favorable visual quality for a given file size and is recommended
for applications where a target bitrate is not necessary, such as in a home environment.
Set `--rc=0` to use this method.

## Notes

The feature settings that are described in this document were compiled at
v2.2.0 of the code and may not reflect the current status of the code. The
description in this document represents an example showing how features would
interact with the SVT architecture. For the most up-to-date settings, it's
recommended to review the section of the code implementing this feature.
