pub(super) struct SubSortFragment<'a, T> {
    pub(super) src: &'a mut [T],
    pub(super) buf: &'a mut [T],
}

pub(super) trait FragmentationByMarks<T> {
    fn fragment_by_marks<'a>(
        &'a mut self,
        buf: &'a mut [T],
        marks: &[usize],
    ) -> Vec<SubSortFragment<'a, T>>;
}

impl<T> FragmentationByMarks<T> for [T] {

    #[inline]
    fn fragment_by_marks<'a>(
        &'a mut self,
        buffer: &'a mut [T],
        marks: &[usize],
    ) -> Vec<SubSortFragment<'a, T>> {
        debug_assert_eq!(self.len(), buffer.len());

        let mut frags = Vec::with_capacity(marks.len() + 1);

        let mut src = self;
        let mut buf = buffer;

        let mut base = 0;
        for &m in marks.iter() {
            debug_assert!(m >= base, "marks must be non-decreasing");
            debug_assert!(m <= base + src.len(), "mark {m} out of bounds");

            let md = m - base;
            let (left_src, right_src) = src.split_at_mut(md);
            let (left_buf, right_buf) = buf.split_at_mut(md);

            frags.push(SubSortFragment {
                src: left_src,
                buf: left_buf,
            });

            src = right_src;
            buf = right_buf;

            base = m;
        }

        if !src.is_empty() {
            frags.push(SubSortFragment { src, buf });
        }

        frags
    }
}
