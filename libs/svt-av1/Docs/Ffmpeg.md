[Top level](../README.md)

# Using SVT-AV1 within ffmpeg

## A Note on ffmpeg Versions

Although ffmpeg has an SVT-AV1 wrapper, its functionality was severely limited
prior to and including ffmpeg version 5.0.X. Any version starting with 5.1.0 will
permit full SVT-AV1 functionality, including passing SVT-AV1 parameters directly
via the `svtav1-params` keyword.

If your ffmpeg version is 5.0.X or lower, we suggest you upgrade to a more recent version or
use the [ffmpeg patch](../ffmpeg_plugin/README.md) included in the [SVT-AV1 repository](https://gitlab.com/AOMediaCodec/SVT-AV1/-/tree/master/ffmpeg_plugin).

## Most Common Options

The parameters that are most frequently changed from one encode to another are

* `crf`. This parameter governs the quality/size trade-off. Higher CRF values will
  result in a final output that takes less space, but begins to lose detail. Lower CRF
  values retain more detail at the cost of larger file sizes. The possible range of CRF
  in SVT-AV1 is 1-70. CRF values are not meant to be equivalent across different
  encoders. A good starting point for 1080p video is `crf=30`.
* `preset`. This parameter governs the efficiency/encode-time trade-off. Lower
  presets will result in an output with better quality for a given file size, but
  will take longer to encode. Higher presets can result in a very fast encode,
  but will make some compromises on visual quality for a given crf value.
* `-g` (in ffmpeg) or `keyint` (in SvtAv1EncApp). These parameters govern how
  many frames will pass before the encoder will add a key frame. Key frames include
  all information about a single image. Other (delta) frames store only
  differences between one frame and another. Key frames are necessary for seeking
  and for error-resilience (in VOD applications). More frequent key frames will
  make the video quicker to seek and more robust, but it will also increase the
  file size. For VOD, a setting a key frame once per second or so is a common
  choice. In other contexts, less frequent key frames (such as 5 or 10 seconds)
  are preferred.
* `film-grain`. Because of its random nature, film grain and CCD noise are
  very difficult to compress. The AV1 specification has the
  capability to produce synthetic noise of varying intensity. It is therefore
  possible for SVT-AV1 to delete film grain and CCD noise and replace it with
  [synthetic grain](Appendix-Film-Grain-Synthesis.md) of the same character, resulting in good bitrate savings while
  retaining subjective visual quality and character. The `film-grain` parameter enables this
  behavior. Setting it to a higher level does so more aggressively. Very high
  levels of denoising can result in the loss of some high-frequency detail, however.
* `pix_fmt`. This parameter can be used to force encoding to 10 or 8 bit color depth. By default
  SVT-AV1 will encode 10-bit sources to 10-bit outputs and 8-bit to 8-bit.
* `tune`. This parameter changes some encoder settings to produce a result
  that is optimized for subjective quality (`tune=0`) or PSNR (`tune=1`). Tuning
  for subjective quality can result in a sharper image and higher psycho-visual fidelity.

The following are some examples of common use cases that utilize ffmpeg.

## Example 1: Fast/Realtime Encoding

For fast encoding, the preset must be sufficiently high that your CPU can
encode without stuttering. Higher presets are faster but less efficient. The
highest preset is 13 (the highest preset intended for human use is 12).

    ffmpeg -i infile.mkv -c:v libsvtav1 -preset 10 -crf 35 -c:a copy outfile.mkv

Since SVT-AV1 is designed to scale well across cores/processors, fast encoding is
best performed on machines with a sufficient number of threads.

## Example 2: Encoding for Personal Use

When encoding for personal use, such as a media server or HTPC, higher efficiency
and *reasonable* encoding times are desirable.

    ffmpeg -i infile.mkv -c:v libsvtav1 -preset 5 -crf 32 -g 240 -pix_fmt yuv420p10le -svtav1-params tune=0:film-grain=8 -c:a copy outfile.mkv

Presets between 4 and 6 offer what many people consider a reasonable trade-off
between quality and encoding time. Encoding with 10-bit depth results in more
accurate colors and fewer artifacts with minimal increase in file size, though the
resulting file may be somewhat more computationally intensive to decode for a given
bitrate.

If higher decoding performance is required, using 10-bit YCbCr encoding will
increase efficiency, so a lower average bitrate can be used, which in turn
improves decoding performance. In addition, passing the parameter
`fast-decode=1` can help (this parameter does not have an effect for all
presets, so check the [parameter description](Parameters.md) for your preset).
Last, for a given bitrate, 8-bit `yuv420p` can sometimes be faster to encode,
albeit at the cost of some fidelity.

The `tune=0` parameter optimizes the encode for subjective visual quality (with higher sharpness),
instead of objective quality (PSNR).

The `film-grain` parameter allows SVT-AV1 to detect and delete film grain from the original video,
and replace it with synthetic grain of the same character, resulting in significant bitrate savings. A
value of 8 is a reasonable starting point for live-action video with a normal amount of grain. Higher
values in the range of 10-15 enable more aggressive use of this technique for video with lots of natural
grain. For 2D animation, lower values in the range of 4-6 are often appropriate. If the original
video does not have natural grain, this parameter can be omitted.

Note that the `crf` range for SVT-AV1 is 1-70, which is a wider range than is found on some popular
open-source encoders. As a result, `crf` values that approximate the visual quality in those encoders
will tend to be higher in SVT-AV1.

Using a higher GOP via the `-g` ffmpeg parameter results in a more efficient
encode in terms of quality per bitrate, at the cost of seeking performance. A
common rule-of-thumb among hobbyists is to use ten times the framerate of the
video, but not more than 300.

## Example 3: Encoding for Video On Demand

For professional VOD applications, the best possible efficiency is often
desired and videos are often split by scenes using third-party tools.

A short GOP size (the `g` parameter) results in better seeking performance and fault-tolerance.

An example use of a single-scene video:

    ffmpeg -i infile.mkv -c:v libsvtav1 -preset 2 -crf 25 -g 24 -pix_fmt yuv420p10le -svtav1-params tune=0:film-grain=8 -c:a copy outfile.mkv

Note that using 8-bit instead may increase decode performance and compatibility.

# Piping from ffmpeg into the standalone encoder

If you are unable to use a version of ffmpeg that is recent enough to pass all required parameters
to SVT-AV1 (or if you want to use a specific version of SVT-AV1 rather than the one included with your version
of ffmpeg), you can use ffmpeg to decode the video and pipe the result to the standalone app
for encoding. Then you can add the audio and video into a final file.

    ffmpeg -i infile.mkv -map 0:v:0 -pix_fmt yuv420p10le -f yuv4mpegpipe -strict -1 - | SvtAv1EncApp -i stdin --preset 5 --keyint 240 --input-depth 10 --crf 32 --film-grain 8 --tune 0 -b outfile.ivf
    ffmpeg -i outfile.ivf -i infile.mkv -map 0:v -map 1:a:0 -c:v copy -c:a copy outfile.mkv

