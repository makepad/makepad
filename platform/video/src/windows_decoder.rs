//! Windows Media Foundation backend for the video FILE decoder seam.
//!
//! Source-reader based: an mp4 path in, decoded NV12 frames + 16-bit PCM
//! audio out. Hardware transforms and advanced video processing are enabled
//! so the OS auto-selects the hardware decoder MFT when one exists
//! (vendor-neutral) and performs any needed format conversion to NV12.

use {
    crate::{
        windows_encoder::{ensure_media_foundation, hr_err, to_wide},
        DecodedAudioChunk, DecodedVideoFrame, VideoFileCodec, VideoFileError, VideoFileInfo,
    },
    windows::{
        core::{GUID, PCWSTR},
        Win32::Media::MediaFoundation::{
            IMFSample, IMFSourceReader, MFAudioFormat_PCM,
            MFCreateAttributes, MFCreateMFByteStreamOnStream, MFCreateMediaType,
            MFCreateSourceReaderFromByteStream, MFCreateSourceReaderFromURL,
            MFMediaType_Audio, MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_HEVC,
            MFVideoFormat_NV12, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
            MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_DEFAULT_STRIDE,
            MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE,
            MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_SUBTYPE,
            MF_PD_DURATION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
            MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
            MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
            MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
            MF_SOURCE_READER_MEDIASOURCE, MF_BYTESTREAM_CONTENT_TYPE,
        },
        Win32::UI::Shell::SHCreateMemStream,
        Win32::System::Com::StructuredStorage::{
            PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
        },
        Win32::System::Variant::{VARENUM, VT_UI8},
    },
};

/// `VT_I8` — the trimmed vendored windows crate generates only the `VARENUM`
/// values it already uses. `IMFSourceReader::SetCurrentPosition` with a NULL
/// time format wants the position as a signed 100ns `LONGLONG`, i.e. VT_I8.
const VT_I8: VARENUM = VARENUM(20);

pub struct WindowsVideoFileDecoder {
    reader: IMFSourceReader,
    info: VideoFileInfo,
    /// Decoded surface dimensions; codecs pad to block/CTU alignment, so the
    /// coded surface can be larger than the display picture in `info`.
    coded_width: u32,
    coded_height: u32,
    video_stride: usize,
    video_eos: bool,
    audio_eos: bool,
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.max(1)
}

/// Read the display picture size from an MF_MT_MINIMUM_DISPLAY_APERTURE blob
/// (an MFVideoArea: two MFOffset {fract:u16, value:i16} + SIZE {cx:i32, cy:i32}).
unsafe fn display_aperture_size(
    media_type: &windows::Win32::Media::MediaFoundation::IMFMediaType,
) -> Option<(u32, u32)> {
    let mut blob = [0u8; 16];
    media_type
        .GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut blob, None)
        .ok()?;
    let cx = i32::from_le_bytes(blob[8..12].try_into().unwrap());
    let cy = i32::from_le_bytes(blob[12..16].try_into().unwrap());
    if cx > 0 && cy > 0 {
        Some((cx as u32, cy as u32))
    } else {
        None
    }
}

impl WindowsVideoFileDecoder {
    pub fn open(path: &str) -> Result<Self, VideoFileError> {
        ensure_media_foundation()?;
        unsafe {
            let attributes = Self::reader_attributes()?;
            let wide_path = to_wide(path);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(wide_path.as_ptr()), &attributes)
                .map_err(|e| hr_err("MFCreateSourceReaderFromURL", e))?;
            Self::from_reader(reader)
        }
    }

    /// Same as [`Self::open`] but over an in-RAM byte stream — the container
    /// never touches the filesystem. The stream is labeled `video/mp4` so
    /// source resolution does not depend on a URL extension.
    pub fn open_bytes(bytes: &[u8]) -> Result<Self, VideoFileError> {
        ensure_media_foundation()?;
        unsafe {
            let attributes = Self::reader_attributes()?;
            let istream = SHCreateMemStream(Some(bytes))
                .ok_or_else(|| VideoFileError::new("SHCreateMemStream returned null"))?;
            let byte_stream = MFCreateMFByteStreamOnStream(&istream)
                .map_err(|e| hr_err("MFCreateMFByteStreamOnStream", e))?;
            use windows::core::Interface;
            if let Ok(stream_attributes) =
                byte_stream.cast::<windows::Win32::Media::MediaFoundation::IMFAttributes>()
            {
                let content_type = to_wide("video/mp4");
                let _ = stream_attributes
                    .SetString(&MF_BYTESTREAM_CONTENT_TYPE, PCWSTR(content_type.as_ptr()));
            }
            let reader = MFCreateSourceReaderFromByteStream(&byte_stream, &attributes)
                .map_err(|e| hr_err("MFCreateSourceReaderFromByteStream", e))?;
            Self::from_reader(reader)
        }
    }

    unsafe fn reader_attributes(
    ) -> Result<windows::Win32::Media::MediaFoundation::IMFAttributes, VideoFileError> {
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 2).map_err(|e| hr_err("MFCreateAttributes", e))?;
            let attributes = attributes.unwrap();
            attributes
                .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                .map_err(|e| hr_err("set MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS", e))?;
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
                .map_err(|e| hr_err("set MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING", e))?;
            Ok(attributes)
        }
    }

    unsafe fn from_reader(reader: IMFSourceReader) -> Result<Self, VideoFileError> {
        unsafe {
            let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let audio_stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

            // Compressed video info from the native type.
            let native = reader
                .GetNativeMediaType(video_stream, 0)
                .map_err(|e| hr_err("no video stream in file", e))?;
            let native_subtype = native
                .GetGUID(&MF_MT_SUBTYPE)
                .map_err(|e| hr_err("get native video subtype", e))?;
            #[allow(non_upper_case_globals)]
            let video_codec = match native_subtype {
                MFVideoFormat_HEVC => Some(VideoFileCodec::H265),
                MFVideoFormat_H264 => Some(VideoFileCodec::H264),
                _ => None,
            };
            let frame_rate = native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or((30 << 32) | 1);
            let mut fps_num = (frame_rate >> 32) as u32;
            let mut fps_den = ((frame_rate & 0xffff_ffff) as u32).max(1);
            let g = gcd(fps_num, fps_den);
            fps_num /= g;
            fps_den /= g;
            // Display picture size: the codec pads the coded surface to block
            // alignment; the true picture lives in the display aperture when
            // one is present (checked on the decoded type below, then the
            // compressed type), else the native frame size.
            let native_size = native.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
            let mut display_width = (native_size >> 32) as u32;
            let mut display_height = (native_size & 0xffff_ffff) as u32;

            // Request NV12 output; the reader inserts decoder + converter.
            let out_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType", e))?;
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| hr_err("set decode major type", e))?;
            out_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(|e| hr_err("set decode subtype NV12", e))?;
            reader
                .SetCurrentMediaType(video_stream, None, &out_type)
                .map_err(|e| hr_err("SetCurrentMediaType(video NV12)", e))?;

            let current = reader
                .GetCurrentMediaType(video_stream)
                .map_err(|e| hr_err("GetCurrentMediaType(video)", e))?;
            let frame_size = current
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|e| hr_err("get decoded frame size", e))?;
            let coded_width = (frame_size >> 32) as u32;
            let coded_height = (frame_size & 0xffff_ffff) as u32;
            if let Some((aw, ah)) =
                display_aperture_size(&current).or_else(|| display_aperture_size(&native))
            {
                display_width = aw;
                display_height = ah;
            }
            let width = if display_width != 0 { display_width.min(coded_width) } else { coded_width };
            let height = if display_height != 0 { display_height.min(coded_height) } else { coded_height };
            let video_stride = current
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|v| (v as i32).unsigned_abs() as usize)
                .unwrap_or(coded_width as usize)
                .max(coded_width as usize);

            // Audio: request 16-bit PCM at the native rate/layout when a
            // stream exists.
            let mut has_audio = false;
            let mut audio_sample_rate = 0u32;
            let mut audio_channels = 0u16;
            if let Ok(native_audio) = reader.GetNativeMediaType(audio_stream, 0) {
                let rate = native_audio
                    .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                    .unwrap_or(48000);
                let channels = native_audio
                    .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                    .unwrap_or(2)
                    .clamp(1, 2);
                let pcm_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType", e))?;
                pcm_type
                    .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                    .map_err(|e| hr_err("set decode audio major type", e))?;
                pcm_type
                    .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                    .map_err(|e| hr_err("set decode audio subtype PCM", e))?;
                pcm_type
                    .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                    .map_err(|e| hr_err("set decode audio bits", e))?;
                pcm_type
                    .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)
                    .map_err(|e| hr_err("set decode audio rate", e))?;
                pcm_type
                    .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)
                    .map_err(|e| hr_err("set decode audio channels", e))?;
                pcm_type
                    .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, channels * 2)
                    .map_err(|e| hr_err("set decode audio block align", e))?;
                reader
                    .SetCurrentMediaType(audio_stream, None, &pcm_type)
                    .map_err(|e| hr_err("SetCurrentMediaType(audio PCM)", e))?;
                let current_audio = reader
                    .GetCurrentMediaType(audio_stream)
                    .map_err(|e| hr_err("GetCurrentMediaType(audio)", e))?;
                audio_sample_rate = current_audio
                    .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                    .unwrap_or(rate);
                audio_channels = current_audio
                    .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                    .unwrap_or(channels) as u16;
                has_audio = true;
            }

            // Duration from the presentation descriptor (VT_UI8, 100ns units).
            let mut duration_100ns = 0i64;
            if let Ok(prop) = reader.GetPresentationAttribute(
                MF_SOURCE_READER_MEDIASOURCE.0 as u32,
                &MF_PD_DURATION,
            ) {
                if prop.Anonymous.Anonymous.vt == VT_UI8 {
                    duration_100ns = prop.Anonymous.Anonymous.Anonymous.uhVal as i64;
                }
            }

            Ok(Self {
                reader,
                coded_width,
                coded_height,
                info: VideoFileInfo {
                    width,
                    height,
                    fps_num,
                    fps_den,
                    duration_100ns,
                    video_codec,
                    video_codec_fourcc: native_subtype.data1,
                    has_audio,
                    audio_sample_rate,
                    audio_channels,
                },
                video_stride,
                video_eos: false,
                audio_eos: false,
            })
        }
    }

    pub fn info(&self) -> &VideoFileInfo {
        &self.info
    }

    /// Refresh coded size/stride after MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.
    unsafe fn refresh_video_type(&mut self) {
        let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        if let Ok(current) = self.reader.GetCurrentMediaType(video_stream) {
            if let Ok(frame_size) = current.GetUINT64(&MF_MT_FRAME_SIZE) {
                self.coded_width = (frame_size >> 32) as u32;
                self.coded_height = (frame_size & 0xffff_ffff) as u32;
                self.info.width = self.info.width.min(self.coded_width).max(1);
                self.info.height = self.info.height.min(self.coded_height).max(1);
            }
            self.video_stride = current
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|v| (v as i32).unsigned_abs() as usize)
                .unwrap_or(self.coded_width as usize)
                .max(self.coded_width as usize);
        }
    }

    /// Read the next sample from a stream, handling stream ticks and media
    /// type changes. Returns None at end of stream.
    unsafe fn read_stream_sample(
        &mut self,
        stream: u32,
        context: &str,
    ) -> Result<Option<(i64, IMFSample)>, VideoFileError> {
        loop {
            let mut flags = 0u32;
            let mut timestamp = 0i64;
            let mut sample: Option<IMFSample> = None;
            self.reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )
                .map_err(|e| hr_err(context, e))?;
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                self.refresh_video_type();
            }
            if let Some(sample) = sample {
                return Ok(Some((timestamp, sample)));
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok(None);
            }
            // No sample and not EOS (e.g. a stream tick): read again.
        }
    }

    pub fn next_frame(&mut self) -> Result<Option<DecodedVideoFrame>, VideoFileError> {
        if self.video_eos {
            return Ok(None);
        }
        unsafe {
            let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let Some((pts_100ns, sample)) =
                self.read_stream_sample(video_stream, "ReadSample(video)")?
            else {
                self.video_eos = true;
                return Ok(None);
            };
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| hr_err("ConvertToContiguousBuffer(video)", e))?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut ptr, None, Some(&mut len))
                .map_err(|e| hr_err("IMFMediaBuffer::Lock(video)", e))?;
            let bytes = std::slice::from_raw_parts(ptr as *const u8, len as usize);

            // Crop the coded surface (stride x coded_height, then interleaved
            // UV) down to a tight display-sized NV12 buffer. The display
            // aperture starts at (0,0) for MP4 content.
            let width = self.info.width as usize;
            let height = self.info.height as usize;
            let coded_height = (self.coded_height as usize).max(height);
            let uv_height = height.div_ceil(2);
            let coded_uv_height = coded_height.div_ceil(2);
            let stride = if self.video_stride * (coded_height + coded_uv_height) <= bytes.len() {
                self.video_stride
            } else {
                width
            };
            let mut nv12 = vec![0u8; width * (height + uv_height)];
            if stride * (coded_height + coded_uv_height) <= bytes.len() {
                for row in 0..height {
                    let src = &bytes[row * stride..row * stride + width];
                    nv12[row * width..row * width + width].copy_from_slice(src);
                }
                for row in 0..uv_height {
                    let src_off = (coded_height + row) * stride;
                    let dst_off = (height + row) * width;
                    nv12[dst_off..dst_off + width]
                        .copy_from_slice(&bytes[src_off..src_off + width]);
                }
            } else {
                let n = nv12.len().min(bytes.len());
                nv12[..n].copy_from_slice(&bytes[..n]);
            }
            let _ = buffer.Unlock();

            Ok(Some(DecodedVideoFrame {
                pts_100ns,
                width: self.info.width,
                height: self.info.height,
                nv12,
            }))
        }
    }

    pub fn next_audio(&mut self) -> Result<Option<DecodedAudioChunk>, VideoFileError> {
        if self.audio_eos || !self.info.has_audio {
            return Ok(None);
        }
        unsafe {
            let audio_stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
            let Some((pts_100ns, sample)) =
                self.read_stream_sample(audio_stream, "ReadSample(audio)")?
            else {
                self.audio_eos = true;
                return Ok(None);
            };
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| hr_err("ConvertToContiguousBuffer(audio)", e))?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut ptr, None, Some(&mut len))
                .map_err(|e| hr_err("IMFMediaBuffer::Lock(audio)", e))?;
            let sample_count = len as usize / 2;
            let mut samples = vec![0i16; sample_count];
            std::ptr::copy_nonoverlapping(
                ptr as *const u8,
                samples.as_mut_ptr() as *mut u8,
                sample_count * 2,
            );
            let _ = buffer.Unlock();

            Ok(Some(DecodedAudioChunk {
                pts_100ns,
                sample_rate: self.info.audio_sample_rate,
                channels: self.info.audio_channels,
                samples,
            }))
        }
    }

    /// Position the demuxer at or before `target_100ns` and re-arm both
    /// streams. Landing exactly on the target is the facade's job: Media
    /// Foundation can only seek to a *sync sample*, so the reader restarts at
    /// the key frame at or before the target and the caller decodes forward
    /// from there.
    pub fn seek_to(&mut self, target_100ns: i64) -> Result<(), VideoFileError> {
        // How a media source answers a request to position past its own end is
        // source-specific — some accept it and report end-of-stream, some fail
        // the call. Clamping inside the file makes it the former everywhere,
        // so a past-the-end seek reaches EOS instead of erroring.
        let position_100ns = if self.info.duration_100ns > 0 {
            target_100ns.min(self.info.duration_100ns - 1)
        } else {
            target_100ns
        };
        unsafe {
            // NULL time format => position is 100ns units as a signed
            // LONGLONG. Built by hand rather than dropped through a
            // conversion so nothing can reinterpret the tag.
            let position = PROPVARIANT {
                Anonymous: PROPVARIANT_0 {
                    Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                        vt: VT_I8,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: PROPVARIANT_0_0_0 {
                            hVal: position_100ns,
                        },
                    }),
                },
            };
            self.reader
                .SetCurrentPosition(&GUID::zeroed(), &position)
                .map_err(|e| hr_err("IMFSourceReader::SetCurrentPosition", e))?;
        }
        // The seek re-arms both streams: whatever end-of-stream we had reached
        // before is no longer where we are.
        self.video_eos = false;
        self.audio_eos = false;
        Ok(())
    }
}
