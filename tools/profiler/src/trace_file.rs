use crate::macho::SymbolResolver;
use makepad_micro_serde::{DeJson, DeJsonErr, DeJsonState, SerJson, SerJsonState};
use makepad_profiler::{Capture, CaptureHeader, LoadedImage, ProfilerError};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct TraceSymbol {
    pub id: u32,
    pub image_index: i32,
    pub start_address: u64,
    pub name: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct TraceThreadSample {
    pub thread_port: u32,
    pub thread_id: u64,
    pub run_state: u32,
    pub pc: u64,
    pub sp: u64,
    pub fp: u64,
    pub raw_frames: Vec<u64>,
    pub frames: Vec<u32>,
    pub complete: bool,
    pub error: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct TraceSample {
    pub timestamp_micros: u64,
    pub suspend_micros: u64,
    pub threads: Vec<TraceThreadSample>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct TraceFile {
    pub header: CaptureHeader,
    pub warnings: Vec<String>,
    pub images: Vec<LoadedImage>,
    pub symbols: Vec<TraceSymbol>,
    pub samples: Vec<TraceSample>,
}

#[derive(Clone, Debug)]
pub struct FunctionRow {
    pub name: String,
    pub exclusive_micros: f64,
    pub exclusive_percent: f64,
    pub inclusive_micros: f64,
    pub inclusive_percent: f64,
    pub blocked_micros: f64,
}

#[derive(Clone, Debug)]
pub struct FoldedStack {
    pub frames: Vec<String>,
    pub weight: u64,
}

#[derive(Clone, Default)]
struct FunctionCounts {
    exclusive_samples: u64,
    inclusive_samples: u64,
    blocked_samples: u64,
}

impl TraceFile {
    pub fn from_capture(capture: Capture) -> Self {
        let resolver = SymbolResolver::from_images(&capture.images);
        let mut warnings = capture.warnings.clone();
        warnings.extend(resolver.warnings().iter().cloned());

        let mut symbols = Vec::<TraceSymbol>::new();
        let mut symbol_ids = HashMap::<(i32, u64, String), u32>::new();
        let mut samples = Vec::<TraceSample>::with_capacity(capture.samples.len());

        for sample in capture.samples {
            let mut threads = Vec::with_capacity(sample.threads.len());
            for thread in sample.threads {
                let mut resolved_frames = Vec::with_capacity(thread.frames.len());
                for &address in &thread.frames {
                    let resolved = resolver.resolve(address);
                    let key = (
                        resolved.image_index.map(|index| index as i32).unwrap_or(-1),
                        resolved.symbol_start,
                        resolved.symbol_name.clone(),
                    );
                    let symbol_id = if let Some(existing) = symbol_ids.get(&key) {
                        *existing
                    } else {
                        let id = symbols.len() as u32;
                        symbols.push(TraceSymbol {
                            id,
                            image_index: key.0,
                            start_address: key.1,
                            name: key.2.clone(),
                        });
                        symbol_ids.insert(key, id);
                        id
                    };
                    resolved_frames.push(symbol_id);
                }

                threads.push(TraceThreadSample {
                    thread_port: thread.thread_port,
                    thread_id: thread.thread_id,
                    run_state: thread.run_state,
                    pc: thread.pc,
                    sp: thread.sp,
                    fp: thread.fp,
                    raw_frames: thread.frames,
                    frames: resolved_frames,
                    complete: thread.complete,
                    error: thread.error,
                });
            }

            samples.push(TraceSample {
                timestamp_micros: sample.timestamp_micros,
                suspend_micros: sample.suspend_micros,
                threads,
            });
        }

        Self {
            header: capture.header,
            warnings,
            images: capture.images,
            symbols,
            samples,
        }
    }

    pub fn read_from_path(path: &Path) -> Result<Self, ProfilerError> {
        let json = std::fs::read_to_string(path).map_err(|err| {
            ProfilerError::new(format!("failed to read trace file {}: {}", path.display(), err))
        })?;
        TraceFile::deserialize_json(&json)
            .map_err(|err| ProfilerError::new(format!("failed to parse trace file: {:?}", err)))
    }

    pub fn write_to_path(&self, path: &Path) -> Result<(), ProfilerError> {
        std::fs::write(path, self.serialize_json()).map_err(|err| {
            ProfilerError::new(format!("failed to write trace file {}: {}", path.display(), err))
        })
    }

    pub fn function_rows(&self, limit: usize) -> Vec<FunctionRow> {
        let mut counts = vec![FunctionCounts::default(); self.symbols.len()];
        let mut total_running_samples = 0u64;

        for sample in &self.samples {
            for thread in &sample.threads {
                if thread.frames.is_empty() {
                    continue;
                }

                let unique_frames: HashSet<u32> = thread.frames.iter().copied().collect();
                match classify_thread(thread, &self.symbols) {
                    Some(ThreadDisposition::Running) => {
                    total_running_samples += 1;
                    if let Some(top) = thread.frames.first() {
                        if let Some(entry) = counts.get_mut(*top as usize) {
                            entry.exclusive_samples += 1;
                        }
                    }
                    for symbol_id in unique_frames {
                        if let Some(entry) = counts.get_mut(symbol_id as usize) {
                            entry.inclusive_samples += 1;
                        }
                    }
                    }
                    Some(ThreadDisposition::Blocked) => {
                        for symbol_id in unique_frames {
                            if let Some(entry) = counts.get_mut(symbol_id as usize) {
                                entry.blocked_samples += 1;
                            }
                        }
                    }
                    None => {}
                }
            }
        }

        let interval_micros = self.header.interval_micros as f64;
        let total_running_micros = total_running_samples as f64 * interval_micros;
        let mut rows = Vec::new();

        for symbol in &self.symbols {
            let Some(count) = counts.get(symbol.id as usize) else {
                continue;
            };
            if count.exclusive_samples == 0 && count.inclusive_samples == 0 && count.blocked_samples == 0 {
                continue;
            }

            let exclusive_micros = count.exclusive_samples as f64 * interval_micros;
            let inclusive_micros = count.inclusive_samples as f64 * interval_micros;
            let blocked_micros = count.blocked_samples as f64 * interval_micros;
            let exclusive_percent = if total_running_micros > 0.0 {
                exclusive_micros * 100.0 / total_running_micros
            } else {
                0.0
            };
            let inclusive_percent = if total_running_micros > 0.0 {
                inclusive_micros * 100.0 / total_running_micros
            } else {
                0.0
            };

            rows.push(FunctionRow {
                name: symbol.name.clone(),
                exclusive_micros,
                exclusive_percent,
                inclusive_micros,
                inclusive_percent,
                blocked_micros,
            });
        }

        rows.sort_by(|left, right| {
            right
                .exclusive_micros
                .partial_cmp(&left.exclusive_micros)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    right
                        .inclusive_micros
                        .partial_cmp(&left.inclusive_micros)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    right
                        .blocked_micros
                        .partial_cmp(&left.blocked_micros)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        rows.truncate(limit);
        rows
    }

    pub fn folded_stacks(&self, include_blocked: bool) -> Vec<FoldedStack> {
        let mut stacks = BTreeMap::<Vec<String>, u64>::new();
        let weight = self.header.interval_micros.max(1);

        for sample in &self.samples {
            for thread in &sample.threads {
                let Some(classification) = classify_thread(thread, &self.symbols) else {
                    continue;
                };
                if classification == ThreadDisposition::Blocked && !include_blocked {
                    continue;
                }

                let mut frames = Vec::with_capacity(thread.frames.len() + 1);
                if classification == ThreadDisposition::Blocked {
                    frames.push("[blocked]".to_string());
                }
                for symbol_id in thread.frames.iter().rev() {
                    frames.push(symbol_name(&self.symbols, *symbol_id).to_string());
                }
                if frames.is_empty() {
                    continue;
                }

                *stacks.entry(frames).or_insert(0) += weight;
            }
        }

        stacks
            .into_iter()
            .map(|(frames, weight)| FoldedStack { frames, weight })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThreadDisposition {
    Running,
    Blocked,
}

fn classify_thread(
    thread: &TraceThreadSample,
    symbols: &[TraceSymbol],
) -> Option<ThreadDisposition> {
    let top = *thread.frames.first()?;
    if looks_blocked_symbol(symbol_name(symbols, top)) {
        Some(ThreadDisposition::Blocked)
    } else {
        Some(ThreadDisposition::Running)
    }
}

fn symbol_name(symbols: &[TraceSymbol], symbol_id: u32) -> &str {
    symbols
        .get(symbol_id as usize)
        .map(|symbol| symbol.name.as_str())
        .unwrap_or("<unknown>")
}

fn looks_blocked_symbol(name: &str) -> bool {
    const BLOCKED_PATTERNS: &[&str] = &[
        "pthread_cond_wait",
        "psynch_cvwait",
        "mach_msg",
        "kevent",
        "poll",
        "select",
        "sleep",
        "nanosleep",
        "usleep",
        "semwait",
        "park",
        "recv",
        "accept",
    ];

    BLOCKED_PATTERNS.iter().any(|pattern| name.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_rows_aggregate_running_and_blocked_samples() {
        let trace = TraceFile {
            header: CaptureHeader {
                schema: "makepad-profiler.capture".to_string(),
                profiler_version: 1,
                platform: "macos".to_string(),
                architecture: "aarch64".to_string(),
                process_id: 42,
                executable: "demo-target".to_string(),
                start_unix_micros: 0,
                duration_micros: 2_000,
                interval_micros: 1_000,
                max_frames: 32,
            },
            warnings: Vec::new(),
            images: Vec::new(),
            symbols: vec![
                TraceSymbol {
                    id: 0,
                    image_index: -1,
                    start_address: 0x1000,
                    name: "render_frame".to_string(),
                },
                TraceSymbol {
                    id: 1,
                    image_index: -1,
                    start_address: 0x2000,
                    name: "helper".to_string(),
                },
                TraceSymbol {
                    id: 2,
                    image_index: -1,
                    start_address: 0x3000,
                    name: "pthread_cond_wait".to_string(),
                },
            ],
            samples: vec![
                TraceSample {
                    timestamp_micros: 0,
                    suspend_micros: 10,
                    threads: vec![
                        TraceThreadSample {
                            thread_port: 1,
                            thread_id: 11,
                            run_state: 1,
                            pc: 0x1000,
                            sp: 0,
                            fp: 0,
                            raw_frames: vec![0x1000, 0x2000],
                            frames: vec![0, 1],
                            complete: true,
                            error: String::new(),
                        },
                        TraceThreadSample {
                            thread_port: 2,
                            thread_id: 22,
                            run_state: 3,
                            pc: 0x3000,
                            sp: 0,
                            fp: 0,
                            raw_frames: vec![0x3000],
                            frames: vec![2],
                            complete: true,
                            error: String::new(),
                        },
                    ],
                },
                TraceSample {
                    timestamp_micros: 1_000,
                    suspend_micros: 10,
                    threads: vec![TraceThreadSample {
                        thread_port: 1,
                        thread_id: 11,
                        run_state: 1,
                        pc: 0x1000,
                        sp: 0,
                        fp: 0,
                        raw_frames: vec![0x1000],
                        frames: vec![0],
                        complete: true,
                        error: String::new(),
                    }],
                },
            ],
        };

        let rows = trace.function_rows(10);

        let top = rows.iter().find(|row| row.name == "render_frame").unwrap();
        assert_eq!(top.exclusive_micros, 2_000.0);
        assert_eq!(top.inclusive_micros, 2_000.0);
        assert_eq!(top.blocked_micros, 0.0);

        let callee = rows.iter().find(|row| row.name == "helper").unwrap();
        assert_eq!(callee.exclusive_micros, 0.0);
        assert_eq!(callee.inclusive_micros, 1_000.0);
        assert_eq!(callee.blocked_micros, 0.0);

        let blocked = rows
            .iter()
            .find(|row| row.name == "pthread_cond_wait")
            .unwrap();
        assert_eq!(blocked.exclusive_micros, 0.0);
        assert_eq!(blocked.inclusive_micros, 0.0);
        assert_eq!(blocked.blocked_micros, 1_000.0);
    }

    #[test]
    fn folded_stacks_prefix_blocked_threads_when_requested() {
        let trace = TraceFile {
            header: CaptureHeader {
                schema: "makepad-profiler.capture".to_string(),
                profiler_version: 1,
                platform: "macos".to_string(),
                architecture: "aarch64".to_string(),
                process_id: 42,
                executable: "demo-target".to_string(),
                start_unix_micros: 0,
                duration_micros: 1_000,
                interval_micros: 1_000,
                max_frames: 32,
            },
            warnings: Vec::new(),
            images: Vec::new(),
            symbols: vec![
                TraceSymbol {
                    id: 0,
                    image_index: -1,
                    start_address: 0x1000,
                    name: "busy_frame".to_string(),
                },
                TraceSymbol {
                    id: 1,
                    image_index: -1,
                    start_address: 0x2000,
                    name: "pthread_cond_wait".to_string(),
                },
            ],
            samples: vec![TraceSample {
                timestamp_micros: 0,
                suspend_micros: 10,
                threads: vec![
                    TraceThreadSample {
                        thread_port: 1,
                        thread_id: 11,
                        run_state: 1,
                        pc: 0x1000,
                        sp: 0,
                        fp: 0,
                        raw_frames: vec![0x1000],
                        frames: vec![0],
                        complete: true,
                        error: String::new(),
                    },
                    TraceThreadSample {
                        thread_port: 2,
                        thread_id: 22,
                        run_state: 3,
                        pc: 0x2000,
                        sp: 0,
                        fp: 0,
                        raw_frames: vec![0x2000],
                        frames: vec![1],
                        complete: true,
                        error: String::new(),
                    },
                ],
            }],
        };

        let running_only = trace.folded_stacks(false);
        assert_eq!(running_only.len(), 1);
        assert_eq!(running_only[0].frames, vec!["busy_frame".to_string()]);

        let include_blocked = trace.folded_stacks(true);
        assert_eq!(include_blocked.len(), 2);
        assert_eq!(
            include_blocked[0].frames,
            vec!["[blocked]".to_string(), "pthread_cond_wait".to_string()]
        );
    }
}
