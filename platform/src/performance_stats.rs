use std::collections::VecDeque;

#[derive(Debug)]
pub struct FrameStats {
    pub occurred_at: f64,
    pub time_spent: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RedrawCauseStats {
    pub redraw_all_requests: u64,
    pub redraw_list_requests: u64,
    pub redraw_list_and_children_requests: u64,
    pub repaint_pass_requests: u64,
    pub next_frame_requests: u64,
    pub timer_requests: u64,
}

pub struct PerformanceStats {
    pub last_frame_time: Option<f64>,
    pub max_frame_times: VecDeque<FrameStats>,
    pub redraw_cause_stats: RedrawCauseStats,
}

impl Default for PerformanceStats {
    fn default() -> Self {
        Self {
            last_frame_time: None,
            max_frame_times: VecDeque::with_capacity(100),
            redraw_cause_stats: RedrawCauseStats::default(),
        }
    }
}

impl PerformanceStats {
    pub fn process_frame_data(&mut self, time: f64) {
        if let Some(previous_time) = self.last_frame_time {
            if self.max_frame_times.len() == 0 {
                self.max_frame_times.push_front(FrameStats {
                    occurred_at: time,
                    time_spent: time - previous_time,
                });
                return;
            }

            let current_period = (time * 10.0) as i64;
            let data_data_period = (self.max_frame_times[0].occurred_at * 10.0) as i64;
            if current_period == data_data_period {
                if self.max_frame_times[0].time_spent < time - previous_time {
                    self.max_frame_times[0].time_spent = time - previous_time;
                }
            } else {
                if self.max_frame_times.len() >= 100 {
                    self.max_frame_times.pop_back();
                }

                self.max_frame_times.push_front(FrameStats {
                    occurred_at: time,
                    time_spent: time - previous_time,
                });
            }
        };
        self.last_frame_time = Some(time);
    }

    pub fn redraw_cause_stats(&self) -> RedrawCauseStats {
        self.redraw_cause_stats
    }

    pub fn reset_redraw_cause_stats(&mut self) {
        self.redraw_cause_stats = RedrawCauseStats::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redraw_cause_stats_can_be_reset() {
        let mut stats = PerformanceStats::default();
        stats.redraw_cause_stats.redraw_all_requests = 2;
        stats.redraw_cause_stats.next_frame_requests = 3;

        assert_eq!(stats.redraw_cause_stats().redraw_all_requests, 2);
        assert_eq!(stats.redraw_cause_stats().next_frame_requests, 3);

        stats.reset_redraw_cause_stats();
        assert_eq!(stats.redraw_cause_stats(), RedrawCauseStats::default());
    }
}
