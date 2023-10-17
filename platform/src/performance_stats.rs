use std::collections::VecDeque;

#[derive(Debug)]
pub struct FrameStats {
    pub occurred_at: f64,
    pub time_spent: f64
}

pub struct PerformanceStats {
    pub last_frame_time: Option<f64>,
    pub max_frame_times: VecDeque<FrameStats>,
    //pub frame_times: VecDeque<FrameStats>
}

impl Default for PerformanceStats {
    fn default() -> Self {
        Self {
            last_frame_time: None,
            max_frame_times: VecDeque::with_capacity(100),
            //frame_times: VecDeque::with_capacity(100000),
        }
    }
}

impl PerformanceStats {
    pub fn process_frame_data(&mut self, time: f64) {
        if let Some(previous_time) = self.last_frame_time {
            // if self.frame_times.len() >= 100000 {
            //     self.frame_times.pop_back();
            // }
            // self.frame_times.push_front(FrameStats{
            //     occurred_at: time,
            //     time_spent: time - previous_time
            // });

            if self.max_frame_times.len() == 0 {
                self.max_frame_times.push_front(FrameStats{
                    occurred_at: time,
                    time_spent: time - previous_time
                });
                return
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

                self.max_frame_times.push_front(FrameStats{
                    occurred_at: time,
                    time_spent: time - previous_time
                });
            }
        };
        self.last_frame_time = Some(time);
    }
}