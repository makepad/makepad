//! Packet-relative uniform blocks shared by the native and web frame builders.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UniformRange {
    pub off: u32,
    pub len: u32,
}

impl UniformRange {
    pub const EMPTY: Self = Self { off: 0, len: 0 };

    pub fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// An aligned byte stream and its frame-local uniform ranges.
pub struct UniformStream {
    bytes: Vec<u8>,
    align: u32,
    max_block: u32,
    oversize_logged: bool,
    epoch: u64,
    // Indexed by shader. Epoch zero denotes a vacant entry.
    scope_cache: Vec<(u64, u64, UniformRange)>,
    last_draw_call: Option<([u32; 4], UniformRange)>,
    last_dyn: Option<(u64, u32, UniformRange)>,
}

impl UniformStream {
    pub fn new(align: u32, max_block: u32) -> Self {
        let mut stream = Self {
            bytes: Vec::new(),
            align: 0,
            max_block: 0,
            oversize_logged: false,
            epoch: 0,
            scope_cache: Vec::new(),
            last_draw_call: None,
            last_dyn: None,
        };
        stream.configure(align, max_block);
        stream
    }

    /// Changing backend limits starts a fresh stream. Zero selects the default capability.
    pub fn configure(&mut self, align: u32, max_block: u32) {
        let align = if align == 0 { 256 } else { align.clamp(16, 256).next_power_of_two() };
        let max_block = if max_block == 0 { 65_536 } else { max_block.min(u32::MAX - 256) };
        if (align, max_block) != (self.align, self.max_block) {
            self.align = align;
            self.max_block = max_block;
            self.begin_frame();
        }
    }

    pub fn align(&self) -> u32 {
        self.align
    }

    /// Invalidates ranges from the previous frame and keeps the backing allocations.
    pub fn begin_frame(&mut self) {
        self.bytes.clear();
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.scope_cache.clear();
            self.epoch = 1;
        }
        self.last_draw_call = None;
        self.last_dyn = None;
    }

    /// Hands off the completed frame's bytes, then invalidates its ranges and retains
    /// whatever buffer the callback leaves behind.
    pub fn finish_frame<R>(&mut self, f: impl FnOnce(&mut Vec<u8>) -> R) -> R {
        let out = f(&mut self.bytes);
        self.begin_frame();
        out
    }

    #[inline]
    fn stride_for(&self, len: usize) -> usize {
        let mask = (self.align as usize) - 1;
        (len.max(16) + mask) & !mask
    }

    #[cold]
    #[inline(never)]
    fn log_oversize(&mut self, actual: usize, clamped: usize) {
        if !self.oversize_logged {
            self.oversize_logged = true;
            crate::warning!("clamping {actual}-byte uniform block to {clamped} bytes");
        }
    }

    #[inline]
    pub fn put_bytes(&mut self, bytes: &[u8]) -> UniformRange {
        if bytes.is_empty() {
            return UniformRange::EMPTY;
        }
        let len = bytes.len().min(self.max_block as usize);
        if len < bytes.len() {
            self.log_oversize(bytes.len(), len);
        }
        let start = self.bytes.len();
        let off = u32::try_from(start).expect("uniform offset exceeds the packet's u32 range");
        let stride = self.stride_for(len);
        self.bytes.reserve(stride);
        // SAFETY: `reserve(stride)` guarantees `stride` writable bytes at `start`,
        // `stride >= len`, and all `stride` bytes are initialized before `set_len`.
        unsafe {
            let dst = self.bytes.as_mut_ptr().add(start);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, len);
            if stride > len {
                std::ptr::write_bytes(dst.add(len), 0, stride - len);
            }
            self.bytes.set_len(start + stride);
        }
        UniformRange { off, len: len as u32 }
    }

    pub fn put_f32s(&mut self, floats: &[f32]) -> UniformRange {
        // SAFETY: f32 has no padding, and the byte view borrows the same initialized storage.
        let bytes = unsafe {
            std::slice::from_raw_parts(floats.as_ptr().cast::<u8>(), std::mem::size_of_val(floats))
        };
        self.put_bytes(bytes)
    }

    /// Packs equal-sized views and returns the first view and the byte stride to the next.
    pub fn put_views_f32s(&mut self, views: &[&[f32]]) -> (UniformRange, u32) {
        let Some((first, rest)) = views.split_first() else {
            return (UniformRange::EMPTY, 0);
        };
        let range = self.put_f32s(first);
        if rest.is_empty() {
            return (range, 0);
        }
        for view in rest {
            assert_eq!(view.len(), first.len(), "pass views must have equal-sized uniforms");
            self.put_f32s(view);
        }
        if range.is_empty() {
            return (UniformRange::EMPTY, 0);
        }
        (range, range.len.max(16).next_multiple_of(self.align))
    }

    /// Reuses a shader's current scope block across draw lists within the frame.
    #[inline]
    pub fn put_scope(&mut self, shader_index: usize, uniforms_gen: u64, floats: &[f32]) -> UniformRange {
        if let Some(&(epoch, generation, range)) = self.scope_cache.get(shader_index) {
            if epoch == self.epoch && generation == uniforms_gen {
                return range;
            }
        }
        self.put_scope_miss(shader_index, uniforms_gen, floats)
    }

    #[cold]
    #[inline(never)]
    fn put_scope_miss(&mut self, shader_index: usize, uniforms_gen: u64, floats: &[f32]) -> UniformRange {
        if shader_index >= self.scope_cache.len() {
            self.scope_cache.resize(shader_index + 1, (0, 0, UniformRange::EMPTY));
        }
        let range = self.put_f32s(floats);
        self.scope_cache[shader_index] = (self.epoch, uniforms_gen, range);
        range
    }

    #[inline]
    pub fn put_draw_call(&mut self, floats: &[f32]) -> UniformRange {
        // the value dedup is for the four-float `DrawCallUniforms`; any other length is placed as is
        let Ok(block) = <&[f32; 4]>::try_from(floats) else {
            self.last_draw_call = None;
            return self.put_f32s(floats);
        };
        let words = block.map(f32::to_bits);
        if let Some((previous, range)) = self.last_draw_call {
            if words == previous {
                return range;
            }
        }
        if self.max_block < 16 {
            let range = self.put_f32s(block);
            self.last_draw_call = Some((words, range));
            return range;
        }
        let start = self.bytes.len();
        let off = u32::try_from(start).expect("uniform offset exceeds the packet's u32 range");
        let stride = self.align as usize;
        self.bytes.reserve(stride);
        // SAFETY: `reserve(stride)` guarantees `stride` writable bytes at `start`,
        // `stride == self.align >= 16`, and all `stride` bytes are initialized before `set_len`.
        unsafe {
            let dst = self.bytes.as_mut_ptr().add(start);
            std::ptr::copy_nonoverlapping(words.as_ptr().cast::<u8>(), dst, 16);
            if stride > 16 {
                std::ptr::write_bytes(dst.add(16), 0, stride - 16);
            }
            self.bytes.set_len(start + stride);
        }
        let range = UniformRange { off, len: 16 };
        self.last_draw_call = Some((words, range));
        range
    }

    /// Places `dyn_uniforms` / `user_uniforms`, skipping empty slices and deduplicating matching
    /// `(uniforms_gen, byte_len)` with the previous draw. Callers must stamp a unique per-draw-call
    /// generation (`Cx::next_uniform_gen()`) so distinct `dyn_uniforms` slices never share a generation.
    #[inline]
    pub fn put_dyn(&mut self, uniforms_gen: u64, floats: &[f32]) -> UniformRange {
        if floats.is_empty() {
            self.last_dyn = None;
            return UniformRange::EMPTY;
        }
        if uniforms_gen == 0 {
            self.last_dyn = None;
            return self.put_f32s(floats);
        }
        let len = std::mem::size_of_val(floats);
        if let Some((generation, byte_len, range)) = self.last_dyn {
            if generation == uniforms_gen && byte_len as usize == len {
                return range;
            }
        }
        let range = self.put_f32s(floats);
        self.last_dyn = u32::try_from(len).ok().map(|len| (uniforms_gen, len, range));
        range
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_stream_alignment_and_dedup() {
        for align in [16, 64, 256] {
            let mut stream = UniformStream::new(align, 64);
            let (pass, stride) = stream.put_views_f32s(&[&[1.0; 5], &[2.0; 5]]);
            assert_eq!(stride, 20u32.next_multiple_of(align));
            let right_eye = UniformRange { off: pass.off + stride, len: pass.len };
            let list = stream.put_f32s(&[3.0]);
            let scope = stream.put_scope(2, 7, &[4.0]);
            let other_scope = stream.put_scope(3, 7, &[5.0]);
            let len = stream.as_bytes().len();
            assert_eq!(stream.put_scope(2, 7, &[4.0]), scope);
            assert_eq!(stream.as_bytes().len(), len);

            let call = [f32::from_bits(0x7fc00001), 0.0, 1.0, 2.0];
            let changed = [call[0], -0.0, 1.0, 2.0];
            let draw = stream.put_draw_call(&call);
            assert_eq!(stream.put_draw_call(&call), draw);
            let next_draw = stream.put_draw_call(&changed);
            let dynamic = stream.put_dyn(8, &[6.0]);
            assert_eq!(stream.put_dyn(8, &[6.0]), dynamic);
            assert_eq!(stream.put_dyn(8, &[]), UniformRange::EMPTY);
            let next_dynamic = stream.put_dyn(8, &[6.0]);

            let mut end = 0;
            for (range, values) in [
                (pass, &[1.0; 5][..]), (right_eye, &[2.0; 5]), (list, &[3.0]),
                (scope, &[4.0]), (other_scope, &[5.0]), (draw, &call), (next_draw, &changed),
                (dynamic, &[6.0]), (next_dynamic, &[6.0]),
            ] {
                assert_eq!(range.off, end, "each distinct block occupies one stride");
                assert_eq!(range.off % align, 0);
                let expected: Vec<_> = values.iter().flat_map(|x| x.to_ne_bytes()).collect();
                assert_eq!(range.len as usize, expected.len());
                let data_end = (range.off + range.len) as usize;
                end += range.len.max(16).next_multiple_of(align);
                assert_eq!(&stream.as_bytes()[range.off as usize..data_end], expected);
                assert!(stream.as_bytes()[data_end..end as usize].iter().all(|&b| b == 0));
            }
            assert_eq!(stream.as_bytes().len(), end as usize);

            let capacity = stream.bytes.capacity();
            stream.begin_frame();
            assert!(stream.is_empty());
            assert_eq!(stream.bytes.capacity(), capacity);
            assert_eq!(stream.put_scope(2, 7, &[4.0]), UniformRange { off: 0, len: 4 });
            assert_eq!(stream.put_draw_call(&changed).off, align);
            assert_eq!(stream.put_dyn(8, &[6.0]).off, 2 * align);
            let clamped = stream.put_bytes(&[0xa5; 65]);
            assert_eq!(clamped.len, 64);
            let block = &stream.as_bytes()[clamped.off as usize..];
            assert_eq!(&block[..64], &[0xa5; 64]);
            assert!(block[64..].iter().all(|&b| b == 0));
            assert_eq!(stream.as_bytes().len() % align as usize, 0);
            let len = stream.as_bytes().len();
            assert_eq!(stream.put_views_f32s(&[&[], &[]]), (UniformRange::EMPTY, 0));
            assert_eq!(stream.as_bytes().len(), len);
        }
    }

    #[test]
    fn uniform_stream_finish_frame_swaps_and_resets() {
        let mut stream = UniformStream::new(16, 64);
        let call = [3.0, 4.0, 5.0, 6.0];
        stream.put_views_f32s(&[&[1.0]]);
        stream.put_scope(2, 7, &[2.0]);
        stream.put_draw_call(&call);
        stream.put_dyn(8, &[7.0]);

        let ptr = stream.as_bytes().as_ptr();
        let packet = stream.finish_frame(std::mem::take);
        let expected = [
            1.0, 0.0, 0.0, 0.0,
            2.0, 0.0, 0.0, 0.0,
            3.0, 4.0, 5.0, 6.0,
            7.0, 0.0, 0.0, 0.0,
        ].map(f32::to_ne_bytes);
        assert_eq!(packet.as_ptr(), ptr);
        assert_eq!(packet.as_slice(), expected.as_flattened());
        assert!(stream.is_empty());

        assert_eq!(stream.put_scope(2, 7, &[2.0]), UniformRange { off: 0, len: 4 });
        assert_eq!(stream.put_draw_call(&call), UniformRange { off: 16, len: 16 });
        assert_eq!(stream.put_dyn(8, &[7.0]), UniformRange { off: 32, len: 4 });
        stream.finish_frame(|_| ());
        assert_eq!(stream.put_draw_call(&call), UniformRange { off: 0, len: 16 });
        assert_eq!(stream.put_dyn(8, &[7.0]), UniformRange { off: 16, len: 4 });
        stream.finish_frame(|_| ());
        assert_eq!(stream.put_dyn(8, &[7.0]), UniformRange { off: 0, len: 4 });

        let mut short = UniformStream::new(16, 8);
        assert_eq!(short.put_draw_call(&call), UniformRange { off: 0, len: 8 });
        let expected = [call[0], call[1], 0.0, 0.0].map(f32::to_ne_bytes);
        assert_eq!(short.as_bytes(), expected.as_flattened());
    }
}
