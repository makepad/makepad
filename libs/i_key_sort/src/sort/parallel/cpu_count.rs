pub(super) struct CPUCount;

impl CPUCount {
    #[inline]
    pub(super) fn count() -> usize {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub(super) fn should_parallel(len: usize) -> Option<usize> {
        const MIN_LEN_PER_TASK_POWER: u32 = 15;
        const MIN_LEN_PER_TASK: usize = 1 << MIN_LEN_PER_TASK_POWER;

        if len < 2 * MIN_LEN_PER_TASK {
            return None;
        }

        let cpu = CPUCount::count();
        if cpu <= 1 {
            return None;
        }

        let tasks = (len >> MIN_LEN_PER_TASK_POWER).min(cpu);
        Some(tasks)
    }

    #[cfg(debug_assertions)]
    #[inline(always)]
    pub(super) fn should_parallel(_: usize) -> Option<usize> {
        Some(CPUCount::count().max(1))
    }
}
