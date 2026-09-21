use crate::whisper::decoder::{self, KvCache};
use crate::whisper::encoder;
use crate::whisper::mel;
use crate::whisper::model::WhisperModel;

/// A transcribed text segment with timestamps.
#[derive(Debug, Clone)]
pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// A segment plus per-word timing read off the decoder's own
/// cross-attention (see `align.rs`). `words` splits `text` exactly like
/// `str::split_whitespace`; it is empty when alignment was not possible for
/// the chunk (no tokens, degenerate audio), never partially filled with
/// wrong text.
#[derive(Debug, Clone)]
pub struct AlignedSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub words: Vec<crate::whisper::align::WordSpan>,
    /// The segment's TEXT token ids, in order — what [`WhisperState::force_align`]
    /// needs to re-align this text against a different (usually narrower)
    /// window without re-transcribing, and therefore without transcription
    /// variance.
    pub tokens: Vec<i32>,
}

/// Parameters for transcription.
#[derive(Debug, Clone)]
pub struct WhisperParams {
    pub language: String,
    pub translate: bool,
    pub no_timestamps: bool,
    pub single_segment: bool,
    pub temperature: f32,
    pub max_tokens: usize,
    pub no_speech_thold: f32,
    pub suppress_blank: bool,
}

impl Default for WhisperParams {
    fn default() -> Self {
        WhisperParams {
            language: "en".into(),
            translate: false,
            no_timestamps: false,
            single_segment: false,
            temperature: 0.0,
            max_tokens: 0,
            no_speech_thold: 0.6,
            suppress_blank: true,
        }
    }
}

/// Mutable state for the transcription process.
pub struct WhisperState {
    kv_cache: KvCache,
}

impl WhisperState {
    pub fn new(model: &WhisperModel) -> Self {
        let n_layers = model.hparams.n_text_layer as usize;
        WhisperState {
            kv_cache: KvCache::new(n_layers),
        }
    }

    /// Transcribe PCM audio (f32, 16kHz, mono) and return text segments.
    pub fn transcribe(
        &mut self,
        model: &WhisperModel,
        samples: &[f32],
        params: &WhisperParams,
    ) -> Vec<Segment> {
        self.transcribe_impl(model, samples, params, false)
            .into_iter()
            .map(|segment| Segment {
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                text: segment.text,
            })
            .collect()
    }

    /// The same transcription, with per-word timestamps from cross-attention
    /// DTW. Slower than [`Self::transcribe`]: capturing attention forces the
    /// plain CPU path for the decoder's cross-attention (the encoder and
    /// everything else keep their accelerators), which is the right trade
    /// for an offline bake and the wrong one for live dictation.
    pub fn transcribe_aligned(
        &mut self,
        model: &WhisperModel,
        samples: &[f32],
        params: &WhisperParams,
    ) -> Vec<AlignedSegment> {
        self.transcribe_impl(model, samples, params, true)
    }

    fn transcribe_impl(
        &mut self,
        model: &WhisperModel,
        samples: &[f32],
        params: &WhisperParams,
        align: bool,
    ) -> Vec<AlignedSegment> {
        let n_ctx = model.hparams.n_audio_ctx as usize;
        let n_mels = model.hparams.n_mels as usize;

        // 1. Compute mel spectrogram
        let _t = std::time::Instant::now();
        let (mel_data, _n_mel, n_mel_len, n_mel_len_org) =
            mel::log_mel_spectrogram(samples, &model.filters, 1);
        crate::whisper::PROF_MEL.fetch_add(
            _t.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        let mut segments: Vec<AlignedSegment> = Vec::new();
        let mut seek = 0usize; // in mel frames
        let heads = align.then(|| crate::whisper::align::AlignmentHeads::for_model(&model.hparams));

        // Use the original (non-30s-padded) mel length as the decode endpoint.
        let mel_end = n_mel_len_org.min(n_mel_len);

        while seek < mel_end {
            // 2. Extract mel chunk: [n_mels, 2*n_ctx] starting at seek
            let chunk_len = 2 * n_ctx;
            let is_final_chunk = seek + chunk_len >= mel_end;
            let mut mel_chunk = vec![0.0f32; n_mels * chunk_len];
            for j in 0..n_mels {
                for i in 0..chunk_len {
                    let mel_idx = seek + i;
                    if mel_idx < n_mel_len {
                        mel_chunk[j * chunk_len + i] = mel_data[j * n_mel_len + mel_idx];
                    }
                    // else: zero (already)
                }
            }

            // 3. Encode
            let _t = std::time::Instant::now();
            let encoder_out = encoder::encode(model, &mel_chunk, n_ctx);
            crate::whisper::PROF_ENCODER.fetch_add(
                _t.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );

            // 4. Pre-compute cross-attention KV
            let _t = std::time::Instant::now();
            let cross_kv = encoder::compute_cross_kv(model, &encoder_out);
            crate::whisper::PROF_CROSS_KV.fetch_add(
                _t.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );

            // 5. Decode tokens
            self.kv_cache.clear();

            // One capture per chunk: rows are appended in decode order, so
            // row (n_prompt + i) is generated token i.
            let mut capture = heads.as_ref().map(|heads| {
                crate::whisper::align::AlignCapture::new(
                    heads,
                    model.hparams.n_text_layer as usize,
                    model.hparams.n_text_head as usize,
                    n_ctx,
                )
            });

            let vocab = &model.vocab;

            // Build initial prompt: [SOT, lang_id, task, (notimestamps?)]
            let mut prompt_tokens: Vec<i32> = Vec::new();
            prompt_tokens.push(vocab.token_sot);

            // Language token (SOT + 1 + lang_id)
            if vocab.is_multilingual() {
                let lang_id = language_id(&params.language);
                prompt_tokens.push(vocab.token_sot + 1 + lang_id);
                if params.translate {
                    prompt_tokens.push(vocab.token_translate);
                } else {
                    prompt_tokens.push(vocab.token_transcribe);
                }
            }

            if params.no_timestamps {
                prompt_tokens.push(vocab.token_not);
            }

            let n_prompt = prompt_tokens.len();
            let positions: Vec<i32> = (0..n_prompt as i32).collect();

            // Decode prompt
            let _t = std::time::Instant::now();
            let logits = decoder::decode(
                model,
                &prompt_tokens,
                &positions,
                &mut self.kv_cache,
                &cross_kv,
                capture.as_mut(),
            );
            crate::whisper::PROF_DECODER.fetch_add(
                _t.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            crate::whisper::PROF_DECODER_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            decoder::advance_kv_cache(&mut self.kv_cache, n_prompt);

            // Check no-speech probability
            let n_vocab = model.hparams.n_vocab as usize;
            let last_logits = &logits.data[(n_prompt - 1) * n_vocab..n_prompt * n_vocab];
            let no_speech_prob = softmax_single(last_logits, vocab.token_nosp as usize);

            if no_speech_prob > params.no_speech_thold {
                // Skip this chunk - no speech detected
                seek += n_ctx * 2;
                continue;
            }

            // 6. Auto-regressive token generation (greedy)
            let mut result_tokens: Vec<TokenData> = Vec::new();
            let max_tokens = if params.max_tokens > 0 {
                params.max_tokens
            } else {
                model.hparams.n_text_ctx as usize / 2
            }
            .min((model.hparams.n_text_ctx as usize).saturating_sub(n_prompt));

            let mut has_ts = false;
            let mut seek_delta: usize = n_ctx * 2; // default: advance full chunk

            for _i in 0..max_tokens {
                if self.kv_cache.n_past >= model.hparams.n_text_ctx as usize {
                    break;
                }
                // Get logits for last token
                let prev_token = if result_tokens.is_empty() {
                    // Use last logits from prompt decode
                    let token =
                        sample_greedy(last_logits, vocab, params, &result_tokens, has_ts, 0);
                    result_tokens.push(token);
                    if token.id == vocab.token_eot {
                        break;
                    }
                    if token.id >= vocab.token_beg {
                        has_ts = true;
                        seek_delta = 2 * (token.id - vocab.token_beg) as usize;
                    }
                    continue;
                } else {
                    result_tokens.last().unwrap().id
                };

                let pos = (self.kv_cache.n_past) as i32;
                let _t = std::time::Instant::now();
                let logits = decoder::decode(
                    model,
                    &[prev_token],
                    &[pos],
                    &mut self.kv_cache,
                    &cross_kv,
                    capture.as_mut(),
                );
                crate::whisper::PROF_DECODER.fetch_add(
                    _t.elapsed().as_nanos() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                crate::whisper::PROF_DECODER_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                decoder::advance_kv_cache(&mut self.kv_cache, 1);

                let cur_logits = &logits.data[0..n_vocab];
                let seek_delta_cur = if has_ts { seek_delta } else { 0 };
                let token = sample_greedy(
                    cur_logits,
                    vocab,
                    params,
                    &result_tokens,
                    has_ts,
                    seek_delta_cur,
                );

                if token.id == vocab.token_eot {
                    break;
                }

                if token.id >= vocab.token_beg {
                    let new_delta = 2 * (token.id - vocab.token_beg) as usize;
                    if has_ts && new_delta < seek_delta {
                        // Don't go backwards
                        break;
                    }
                    seek_delta = new_delta;
                    has_ts = true;
                }

                result_tokens.push(token);
            }

            // Mirror whisper.cpp behavior: if decoding ends with a single trailing timestamp,
            // advance by the whole chunk to avoid re-decoding the same tail window.
            let single_timestamp_ending = result_tokens.len() > 1
                && result_tokens[result_tokens.len() - 2].id < vocab.token_beg
                && result_tokens[result_tokens.len() - 1].id > vocab.token_beg;
            if single_timestamp_ending {
                seek_delta = std::cmp::min(mel_end - seek, n_ctx * 2);
            }
            if is_final_chunk {
                seek_delta = mel_end - seek;
            }

            // 7. Word alignment: DTW over the generated tokens' captured
            // cross-attention, against only the encoder frames that cover
            // real audio (decoding continues over the zero padding, but the
            // model was not listening to it).
            let n_prompt_rows = n_prompt;
            let alignment = capture.as_ref().and_then(|capture| {
                let n_generated = result_tokens
                    .len()
                    .min(capture.n_rows.saturating_sub(n_prompt_rows));
                let n_frames = (mel_end - seek).min(2 * n_ctx).div_ceil(2);
                crate::whisper::align::align_rows(
                    capture,
                    n_prompt_rows,
                    n_prompt_rows + n_generated,
                    n_frames,
                )
            });

            // 8. Convert tokens to segments
            let chunk_start_ms = (seek as f64 * 10.0) as i64; // mel frames to ms (each frame = 10ms)
            let words_for = |tokens: &[(usize, &str)]| -> Vec<crate::whisper::align::WordSpan> {
                match &alignment {
                    Some(alignment) => {
                        crate::whisper::align::words_from_tokens(tokens, alignment, chunk_start_ms)
                    }
                    None => Vec::new(),
                }
            };

            if params.no_timestamps || params.single_segment {
                // Single segment for entire chunk
                let mut seg_tokens: Vec<(usize, &str)> = Vec::new();
                let mut seg_ids: Vec<i32> = Vec::new();
                for (idx, token) in result_tokens.iter().enumerate() {
                    if token.id < vocab.token_eot {
                        seg_tokens.push((idx, vocab.token_to_str(token.id)));
                        seg_ids.push(token.id);
                    }
                }
                let text: String = seg_tokens.iter().map(|(_, text)| *text).collect();
                if !text.is_empty() {
                    let end_ms = chunk_start_ms + (seek_delta as f64 * 10.0) as i64;
                    segments.push(AlignedSegment {
                        start_ms: chunk_start_ms,
                        end_ms,
                        text,
                        words: words_for(&seg_tokens),
                        tokens: seg_ids,
                    });
                }
            } else {
                // Split on timestamp tokens
                let mut seg_start = chunk_start_ms;
                let mut seg_text = String::new();
                let mut seg_tokens: Vec<(usize, &str)> = Vec::new();
                let mut seg_ids: Vec<i32> = Vec::new();

                for (idx, token) in result_tokens.iter().enumerate() {
                    if token.id >= vocab.token_beg {
                        let ts_ms = chunk_start_ms + (2 * (token.id - vocab.token_beg) as i64) * 10;

                        if !seg_text.is_empty() {
                            segments.push(AlignedSegment {
                                start_ms: seg_start,
                                end_ms: ts_ms,
                                text: seg_text.clone(),
                                words: words_for(&seg_tokens),
                                tokens: seg_ids.clone(),
                            });
                        }
                        seg_start = ts_ms;
                        seg_text.clear();
                        seg_tokens.clear();
                        seg_ids.clear();
                    } else if token.id < vocab.token_eot {
                        seg_text.push_str(vocab.token_to_str(token.id));
                        seg_tokens.push((idx, vocab.token_to_str(token.id)));
                        seg_ids.push(token.id);
                    }
                }

                // Remaining text after last timestamp
                if !seg_text.is_empty() {
                    let end_ms = chunk_start_ms + (seek_delta as f64 * 10.0) as i64;
                    segments.push(AlignedSegment {
                        start_ms: seg_start,
                        end_ms,
                        text: seg_text,
                        words: words_for(&seg_tokens),
                        tokens: seg_ids,
                    });
                }
            }

            // Advance seek
            let seek_step = std::cmp::min(seek_delta, mel_end.saturating_sub(seek));
            if seek_step == 0 {
                break;
            }
            seek += seek_step;
        }

        segments
    }

    /// Teacher-forced alignment: audio window in, the TEXT tokens already
    /// known to be spoken in it, word times out. One batched decoder pass
    /// with attention capture — no sampling, no transcription variance —
    /// then the same DTW as [`Self::transcribe_aligned`]. Word times are
    /// window-relative milliseconds; the caller adds the window's start.
    ///
    /// The token sequence is bracketed by the `no_timestamps` prompt token
    /// and an `eot`, and BOTH are kept as DTW rows: the path is pinned to
    /// the window's corners, so without them the first word would be dragged
    /// to frame zero and the last held to the window's end. The brackets
    /// absorb the leading and trailing non-speech instead — which is
    /// exactly what a karaoke line window has on either side.
    ///
    /// `None` when the text cannot fit the decoder context or the window is
    /// too short to align against.
    pub fn force_align(
        &mut self,
        model: &WhisperModel,
        samples: &[f32],
        tokens: &[i32],
        language: &str,
    ) -> Option<Vec<crate::whisper::align::WordSpan>> {
        if tokens.is_empty() || samples.is_empty() {
            return None;
        }
        let n_ctx = model.hparams.n_audio_ctx as usize;
        let n_mels = model.hparams.n_mels as usize;
        let vocab = &model.vocab;
        let max_samples = mel::WHISPER_SAMPLE_RATE * mel::WHISPER_CHUNK_SIZE;
        let samples = &samples[..samples.len().min(max_samples)];

        let (mel_data, _n_mel, n_mel_len, n_mel_len_org) =
            mel::log_mel_spectrogram(samples, &model.filters, 1);
        let chunk_len = 2 * n_ctx;
        let mut mel_chunk = vec![0.0f32; n_mels * chunk_len];
        for j in 0..n_mels {
            for i in 0..chunk_len.min(n_mel_len) {
                mel_chunk[j * chunk_len + i] = mel_data[j * n_mel_len + i];
            }
        }
        let encoder_out = encoder::encode(model, &mel_chunk, n_ctx);
        let cross_kv = encoder::compute_cross_kv(model, &encoder_out);

        let mut sequence: Vec<i32> = vec![vocab.token_sot];
        if vocab.is_multilingual() {
            sequence.push(vocab.token_sot + 1 + language_id(language));
            sequence.push(vocab.token_transcribe);
        }
        sequence.push(vocab.token_not);
        let n_prompt = sequence.len();
        sequence.extend_from_slice(tokens);
        sequence.push(vocab.token_eot);
        if sequence.len() > model.hparams.n_text_ctx as usize {
            return None;
        }
        let positions: Vec<i32> = (0..sequence.len() as i32).collect();

        let mut capture = crate::whisper::align::AlignCapture::new(
            &crate::whisper::align::AlignmentHeads::for_model(&model.hparams),
            model.hparams.n_text_layer as usize,
            model.hparams.n_text_head as usize,
            n_ctx,
        );
        self.kv_cache.clear();
        let _ = decoder::decode(
            model,
            &sequence,
            &positions,
            &mut self.kv_cache,
            &cross_kv,
            Some(&mut capture),
        );
        self.kv_cache.clear();

        let n_frames = n_mel_len_org.min(n_mel_len).min(chunk_len).div_ceil(2);
        // Rows: [not] + text tokens + [eot]; text rows are offset by one.
        let alignment = crate::whisper::align::align_rows(
            &capture,
            n_prompt - 1,
            n_prompt + tokens.len() + 1,
            n_frames,
        )?;
        let texts: Vec<(usize, &str)> = tokens
            .iter()
            .enumerate()
            .map(|(index, id)| (index + 1, vocab.token_to_str(*id)))
            .collect();
        Some(crate::whisper::align::words_from_tokens(&texts, &alignment, 0))
    }
}

#[derive(Debug, Clone, Copy)]
struct TokenData {
    id: i32,
}

fn sample_greedy(
    logits: &[f32],
    vocab: &crate::whisper::model::Vocab,
    params: &WhisperParams,
    prev_tokens: &[TokenData],
    has_ts: bool,
    seek_delta: usize,
) -> TokenData {
    let n_vocab = logits.len();
    let mut logits = logits.to_vec();

    // Suppress special tokens
    logits[vocab.token_not as usize] = f32::NEG_INFINITY;
    logits[vocab.token_sot as usize] = f32::NEG_INFINITY;
    logits[vocab.token_nosp as usize] = f32::NEG_INFINITY;
    logits[vocab.token_translate as usize] = f32::NEG_INFINITY;
    logits[vocab.token_transcribe as usize] = f32::NEG_INFINITY;
    logits[vocab.token_prev as usize] = f32::NEG_INFINITY;
    logits[vocab.token_solm as usize] = f32::NEG_INFINITY;

    // Suppress blank at start
    if params.suppress_blank && prev_tokens.is_empty() {
        logits[vocab.token_eot as usize] = f32::NEG_INFINITY;
        if let Some(&space_id) = vocab.token_to_id.get(" ") {
            logits[space_id as usize] = f32::NEG_INFINITY;
        }
    }

    // Timestamp pairing constraints
    if !prev_tokens.is_empty() {
        let last = prev_tokens.last().unwrap().id;
        let last_was_ts = last >= vocab.token_beg;

        if last_was_ts {
            let penultimate_was_ts =
                prev_tokens.len() < 2 || prev_tokens[prev_tokens.len() - 2].id >= vocab.token_beg;
            if penultimate_was_ts {
                // Two timestamps in a row: suppress all timestamp tokens
                for i in vocab.token_beg as usize..n_vocab {
                    logits[i] = f32::NEG_INFINITY;
                }
            } else {
                // After text+timestamp: suppress all text tokens
                for i in 0..vocab.token_eot as usize {
                    logits[i] = f32::NEG_INFINITY;
                }
            }
        }
    }

    // Enforce monotonic timestamps
    if has_ts && !params.no_timestamps {
        let tid0 = seek_delta / 2;
        for i in vocab.token_beg as usize..vocab.token_beg as usize + tid0 {
            if i < n_vocab {
                logits[i] = f32::NEG_INFINITY;
            }
        }
    }

    // Temperature scaling
    if params.temperature > 0.0 {
        for l in &mut logits {
            *l /= params.temperature;
        }
    }

    // Greedy: pick argmax
    let mut best_id = 0;
    let mut best_val = f32::NEG_INFINITY;
    for (i, &v) in logits.iter().enumerate() {
        if v > best_val {
            best_val = v;
            best_id = i;
        }
    }

    TokenData { id: best_id as i32 }
}

/// Compute softmax probability for a single token.
fn softmax_single(logits: &[f32], idx: usize) -> f32 {
    let max_val = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for &v in logits {
        sum += (v - max_val).exp();
    }
    (logits[idx] - max_val).exp() / sum
}

fn language_id(lang: &str) -> i32 {
    // Common language codes. For a full implementation this would be a complete table.
    match lang {
        "en" => 0,
        "zh" => 1,
        "de" => 2,
        "es" => 3,
        "ru" => 4,
        "ko" => 5,
        "fr" => 6,
        "ja" => 7,
        "pt" => 8,
        "tr" => 9,
        "pl" => 10,
        "ca" => 11,
        "nl" => 12,
        "ar" => 13,
        "sv" => 14,
        "it" => 15,
        "id" => 16,
        "hi" => 17,
        "fi" => 18,
        "vi" => 19,
        "he" => 20,
        "uk" => 21,
        "el" => 22,
        "ms" => 23,
        "cs" => 24,
        "ro" => 25,
        "da" => 26,
        "hu" => 27,
        "ta" => 28,
        "no" => 29,
        "th" => 30,
        "ur" => 31,
        "hr" => 32,
        "bg" => 33,
        "lt" => 34,
        "la" => 35,
        "mi" => 36,
        "ml" => 37,
        "cy" => 38,
        "sk" => 39,
        "te" => 40,
        "fa" => 41,
        "lv" => 42,
        "bn" => 43,
        "sr" => 44,
        "az" => 45,
        "sl" => 46,
        "kn" => 47,
        "et" => 48,
        "mk" => 49,
        "br" => 50,
        "eu" => 51,
        "is" => 52,
        "hy" => 53,
        "ne" => 54,
        "mn" => 55,
        "bs" => 56,
        "kk" => 57,
        "sq" => 58,
        "sw" => 59,
        "gl" => 60,
        "mr" => 61,
        "pa" => 62,
        "si" => 63,
        "km" => 64,
        "sn" => 65,
        "yo" => 66,
        "so" => 67,
        "af" => 68,
        "oc" => 69,
        "ka" => 70,
        "be" => 71,
        "tg" => 72,
        "sd" => 73,
        "gu" => 74,
        "am" => 75,
        "yi" => 76,
        "lo" => 77,
        "uz" => 78,
        "fo" => 79,
        "ht" => 80,
        "ps" => 81,
        "tk" => 82,
        "nn" => 83,
        "mt" => 84,
        "sa" => 85,
        "lb" => 86,
        "my" => 87,
        "bo" => 88,
        "tl" => 89,
        "mg" => 90,
        "as" => 91,
        "tt" => 92,
        "haw" => 93,
        "ln" => 94,
        "ha" => 95,
        "ba" => 96,
        "jw" => 97,
        "su" => 98,
        _ => 0, // default to english
    }
}
