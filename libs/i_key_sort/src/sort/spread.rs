use alloc::vec::Vec;
use core::mem::MaybeUninit;
use crate::sort::bin_layout::BinLayout;
use crate::sort::key::{KeyFn, SortKey};
use crate::sort::mapper::Mapper;

impl<K: SortKey> BinLayout<K> {
    #[inline(always)]
    pub(crate) fn spread_with_uninit_buffer<T: Copy, F: KeyFn<T, K>>(
        &self,
        src: &mut [T],
        buf: &mut Vec<T>,
        key: F,
    ) -> Mapper {
        buf.clear();

        let need = src.len();

        if buf.capacity() < need {
            buf.reserve(need);
        }

        let scratch: &mut [MaybeUninit<T>] = &mut buf.spare_capacity_mut()[..need];

        let mut mapper = Mapper::new(self.count());
        for a in src.iter() {
            mapper.inc_bin_count(self.index(key(a)));
        }
        mapper.init_indices();

        for val in src.iter() {
            let index = mapper.next_index(self.index(key(val)));
            unsafe { scratch.get_unchecked_mut(index).write(*val); }
        }

        #[allow(clippy::uninit_vec)]
        unsafe { buf.set_len(need); }

        mapper
    }

    #[inline(always)]
    pub(crate) fn spread_with_buffer<T: Copy, F: KeyFn<T, K>>(
        &self,
        src: &mut [T],
        buf: &mut [T],
        key: F,
    ) -> Mapper {
        let mut mapper = Mapper::new(self.count());
        for a in src.iter() {
            mapper.inc_bin_count(self.index(key(a)));
        }

        mapper.init_indices();

        for val in src.iter() {
            let index = mapper.next_index(self.index(key(val)));
            unsafe {
                *buf.get_unchecked_mut(index) = *val;
            }
        }

        mapper
    }
}
