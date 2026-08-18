//! Deterministic point selection and Fourier conditioning for SkinTokens.
//!
//! The two neural conditioners deliberately use different sampling seeds and
//! positional embeddings.  SkinVAE uses the request seed and its PMPE
//! embedding; Michelangelo always uses seed zero in eval mode and the released
//! checkpoint explicitly disables the optional pi frequency multiplier. Both
//! use NumPy's `default_rng` (PCG64) followed
//! by the repository's deterministic farthest-point sampler.  Keeping this in
//! native code makes a fixed request independent of NumPy/Python versions.

use crate::{DiffusionError, Result};

pub const SKIN_TOKENS_CONDITION_CHANNELS: usize = 6;
pub const SKIN_TOKENS_FOURIER_CHANNELS: usize = 51;
pub const SKIN_TOKENS_EMBEDDED_CONDITION_CHANNELS: usize = 54;

/// Which of the released model's two continuous conditioners is being built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkinTokensConditionKind {
    SkinVae,
    Michelangelo,
}

impl SkinTokensConditionKind {
    pub const fn tokens(self) -> usize {
        match self {
            Self::SkinVae => 384,
            Self::Michelangelo => 512,
        }
    }

    pub const fn selection_seed(self, request_seed: u64) -> u64 {
        match self {
            Self::SkinVae => request_seed,
            // `CrossAttentionEncoder._forward` uses default_rng(seed=0) in
            // eval mode, independent of the request seed.
            Self::Michelangelo => 0,
        }
    }
}

/// Observable sampling boundary shared with the official parity oracle.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinTokensConditionSelection {
    /// The `4 * tokens` indices returned by NumPy choice.
    pub candidate_indices: Vec<usize>,
    /// Indices into `candidate_indices`, returned by FPS.
    pub fps_indices: Vec<usize>,
    /// Selected point+normal rows in token-major order (`tokens * 6`).
    pub selected: Vec<f32>,
}

/// Build the deterministic query rows consumed by one conditioner.
pub fn select_condition_rows(
    condition: &[f32],
    request_seed: u64,
    kind: SkinTokensConditionKind,
) -> Result<SkinTokensConditionSelection> {
    if condition.len() % SKIN_TOKENS_CONDITION_CHANNELS != 0 {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens condition has {} scalars, not Nx{}",
            condition.len(),
            SKIN_TOKENS_CONDITION_CHANNELS,
        )));
    }
    let population = condition.len() / SKIN_TOKENS_CONDITION_CHANNELS;
    let tokens = kind.tokens();
    let candidates = tokens * 4;
    if population < candidates {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens condition has {population} rows; deterministic inference requires at least {candidates}",
        )));
    }
    let candidate_indices = numpy_choice_without_replacement(
        population,
        candidates,
        kind.selection_seed(request_seed),
    )?;
    let mut candidate_positions = Vec::with_capacity(candidates * 3);
    for &index in &candidate_indices {
        let base = index * SKIN_TOKENS_CONDITION_CHANNELS;
        candidate_positions.extend_from_slice(&condition[base..base + 3]);
    }
    let fps_indices = farthest_point_indices(&candidate_positions, tokens)?;
    let mut selected = Vec::with_capacity(tokens * SKIN_TOKENS_CONDITION_CHANNELS);
    for &local_index in &fps_indices {
        let index = candidate_indices[local_index];
        let base = index * SKIN_TOKENS_CONDITION_CHANNELS;
        selected.extend_from_slice(
            &condition[base..base + SKIN_TOKENS_CONDITION_CHANNELS],
        );
    }
    Ok(SkinTokensConditionSelection {
        candidate_indices,
        fps_indices,
        selected,
    })
}

/// Embed every point+normal row to the 54 channels consumed by the model.
pub fn embed_condition_rows(
    condition: &[f32],
    kind: SkinTokensConditionKind,
) -> Result<Vec<f32>> {
    if condition.len() % SKIN_TOKENS_CONDITION_CHANNELS != 0 {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens condition has {} scalars, not Nx{}",
            condition.len(),
            SKIN_TOKENS_CONDITION_CHANNELS,
        )));
    }
    let rows = condition.len() / SKIN_TOKENS_CONDITION_CHANNELS;
    let mut output = Vec::with_capacity(rows * SKIN_TOKENS_EMBEDDED_CONDITION_CHANNELS);
    for row in condition.chunks_exact(SKIN_TOKENS_CONDITION_CHANNELS) {
        match kind {
            SkinTokensConditionKind::SkinVae => embed_vae_position(&row[..3], &mut output),
            SkinTokensConditionKind::Michelangelo => {
                embed_michelangelo_position(&row[..3], &mut output)
            }
        }
        output.extend_from_slice(&row[3..6]);
    }
    Ok(output)
}

fn embed_vae_position(position: &[f32], output: &mut Vec<f32>) {
    debug_assert_eq!(position.len(), 3);
    // FrequencyPositionalEmbedding(num_freqs=8, include_pi=True,
    // use_pmpe=True). TokenRig moves the complete VAE to BF16, including the
    // registered frequency and phase buffers, while the input coordinates
    // remain f32. Preserve that easily missed quantization boundary.
    const POWERS: [f32; 8] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0];
    let frequencies = POWERS.map(|power| round_to_bf16(power * std::f32::consts::PI));
    let mut phase_constants = [0.0f32; 8];
    for (index, phase) in phase_constants.iter_mut().enumerate() {
        let fraction = (index + 1) as f32 / 8.0;
        *phase = round_to_bf16(
            (8.0f32.powf(1.0 - fraction) + fraction) * (2.0 * std::f32::consts::PI),
        );
    }
    output.extend_from_slice(position);
    for &coordinate in position {
        for (index, &frequency) in frequencies.iter().enumerate() {
            let embedded = coordinate * frequency;
            let phase = coordinate * (0.5 * std::f32::consts::PI) + phase_constants[index];
            output.push(embedded.sin() + phase.sin());
        }
    }
    for &coordinate in position {
        for (index, &frequency) in frequencies.iter().enumerate() {
            let embedded = coordinate * frequency;
            let phase = coordinate * (0.5 * std::f32::consts::PI) + phase_constants[index];
            output.push(embedded.cos() + phase.cos());
        }
    }
}

fn embed_michelangelo_position(position: &[f32], output: &mut Vec<f32>) {
    debug_assert_eq!(position.len(), 3);
    // The class default is `include_pi=True`, but this checkpoint's serialized
    // mesh-encoder config overrides it to false. This was easy to miss because
    // the frequency buffer is non-persistent and therefore absent from the
    // state dict. The resulting frequencies are exact powers of two.
    const POWERS: [f32; 8] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0];
    output.extend_from_slice(position);
    for &coordinate in position {
        for &power in &POWERS {
            output.push((coordinate * power).sin());
        }
    }
    for &coordinate in position {
        for &power in &POWERS {
            output.push((coordinate * power).cos());
        }
    }
}

fn round_to_bf16(value: f32) -> f32 {
    let bits = value.to_bits();
    let rounded = bits.wrapping_add(0x7fff + ((bits >> 16) & 1));
    f32::from_bits(rounded & 0xffff_0000)
}

/// Official repository FPS: start at candidate zero, update squared f32
/// distances, and choose the first maximum.  This is intentionally O(K*T):
/// K is at most 2048 and T at most 512, dwarfed by the following projection.
pub fn farthest_point_indices(positions: &[f32], samples: usize) -> Result<Vec<usize>> {
    if positions.len() % 3 != 0 {
        return Err(DiffusionError::workflow(
            "SkinTokens FPS positions are not Nx3",
        ));
    }
    let points = positions.len() / 3;
    if samples == 0 || samples > points {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens FPS requested {samples} from {points} points",
        )));
    }
    let mut distances = vec![f32::INFINITY; points];
    let mut selected = Vec::with_capacity(samples);
    let mut farthest = 0usize;
    for _ in 0..samples {
        selected.push(farthest);
        let centroid = &positions[farthest * 3..farthest * 3 + 3];
        for (index, point) in positions.chunks_exact(3).enumerate() {
            // Spell the reduction in the same order as torch.sum(dim=-1).
            let dx = point[0] - centroid[0];
            let dy = point[1] - centroid[1];
            let dz = point[2] - centroid[2];
            let distance = (dx * dx + dy * dy) + dz * dz;
            if distance < distances[index] {
                distances[index] = distance;
            }
        }
        let mut maximum = distances[0];
        farthest = 0;
        for (index, &distance) in distances.iter().enumerate().skip(1) {
            // torch.argmax returns the first maximum.
            if distance > maximum {
                maximum = distance;
                farthest = index;
            }
        }
    }
    Ok(selected)
}

/// NumPy 1.26 `Generator.choice(population, size, replace=False)` for the
/// uniform, shuffled case used here.  The production sizes select NumPy's
/// tail-shuffle branch; the Floyd branch is also implemented so this helper is
/// well-defined for smaller tests and future token-count changes.
pub fn numpy_choice_without_replacement(
    population: usize,
    size: usize,
    seed: u64,
) -> Result<Vec<usize>> {
    if size > population {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens cannot choose {size} rows without replacement from {population}",
        )));
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut rng = NumpyPcg64::new(seed);
    if population > 10_000 && size > population / 50 {
        let mut indices = (0..population).collect::<Vec<_>>();
        let first = population.saturating_sub(size).max(1);
        for index in (first..population).rev() {
            let swap = rng.bounded_u64(index as u64) as usize;
            indices.swap(index, swap);
        }
        Ok(indices[population - size..].to_vec())
    } else {
        // Exact Floyd hash-table branch from NumPy's Generator.choice.
        let requested = size as u64;
        let target = (1.2 * size as f64) as u64;
        let mask = bit_mask_at_least(target);
        let mut table = vec![u64::MAX; (mask + 1) as usize];
        let mut output = Vec::with_capacity(size);
        for index in population - size..population {
            let value = rng.bounded_u64(index as u64);
            let mut location = value & mask;
            while table[location as usize] != u64::MAX
                && table[location as usize] != value
            {
                location = (location + 1) & mask;
            }
            if table[location as usize] == u64::MAX {
                table[location as usize] = value;
                output.push(value as usize);
            } else {
                location = index as u64 & mask;
                while table[location as usize] != u64::MAX {
                    location = (location + 1) & mask;
                }
                table[location as usize] = index as u64;
                output.push(index);
            }
        }
        debug_assert_eq!(output.len() as u64, requested);
        for index in (1..size).rev() {
            let swap = rng.bounded_u64(index as u64) as usize;
            output.swap(index, swap);
        }
        Ok(output)
    }
}

fn bit_mask_at_least(mut value: u64) -> u64 {
    value |= value >> 1;
    value |= value >> 2;
    value |= value >> 4;
    value |= value >> 8;
    value |= value >> 16;
    value |= value >> 32;
    value
}

/// Minimal NumPy SeedSequence + PCG XSL RR 128/64 implementation.
struct NumpyPcg64 {
    state: u128,
    increment: u128,
    cached_high_u32: Option<u32>,
}

impl NumpyPcg64 {
    const MULTIPLIER: u128 =
        ((2_549_297_995_355_413_924u128) << 64) | 4_865_540_595_714_422_341u128;

    fn new(seed: u64) -> Self {
        let words = seed_sequence_state(seed);
        let initial_state = ((words[0] as u128) << 64) | words[1] as u128;
        let initial_sequence = ((words[2] as u128) << 64) | words[3] as u128;
        let increment = (initial_sequence << 1) | 1;
        let mut value = Self {
            state: 0,
            increment,
            cached_high_u32: None,
        };
        value.step();
        value.state = value.state.wrapping_add(initial_state);
        value.step();
        value
    }

    fn step(&mut self) {
        self.state = self
            .state
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(self.increment);
    }

    fn next_u64(&mut self) -> u64 {
        // PCG64 advances first, then applies XSL-RR to the new state.
        self.step();
        let high = (self.state >> 64) as u64;
        let low = self.state as u64;
        (high ^ low).rotate_right((high >> 58) as u32)
    }

    fn next_u32(&mut self) -> u32 {
        if let Some(value) = self.cached_high_u32.take() {
            return value;
        }
        let value = self.next_u64();
        self.cached_high_u32 = Some((value >> 32) as u32);
        value as u32
    }

    /// Inclusive `[0, upper]`, NumPy's 32-bit Lemire path for our ranges.
    fn bounded_u64(&mut self, upper: u64) -> u64 {
        if upper == 0 {
            return 0;
        }
        if upper <= u32::MAX as u64 {
            let upper = upper as u32;
            if upper == u32::MAX {
                return self.next_u32() as u64;
            }
            let range = upper.wrapping_add(1);
            let mut product = self.next_u32() as u64 * range as u64;
            let mut leftover = product as u32;
            if leftover < range {
                let threshold = (u32::MAX - upper) % range;
                while leftover < threshold {
                    product = self.next_u32() as u64 * range as u64;
                    leftover = product as u32;
                }
            }
            product >> 32
        } else {
            let range = upper.wrapping_add(1);
            let mut product = self.next_u64() as u128 * range as u128;
            let mut leftover = product as u64;
            if leftover < range {
                let threshold = (u64::MAX - upper) % range;
                while leftover < threshold {
                    product = self.next_u64() as u128 * range as u128;
                    leftover = product as u64;
                }
            }
            (product >> 64) as u64
        }
    }
}

fn seed_sequence_state(seed: u64) -> [u64; 4] {
    const INIT_A: u32 = 0x43b0_d7e5;
    const MULT_A: u32 = 0x931e_8875;
    const INIT_B: u32 = 0x8b51_f9dd;
    const MULT_B: u32 = 0x58f3_8ded;
    const MIX_MULT_L: u32 = 0xca01_f9dd;
    const MIX_MULT_R: u32 = 0x4973_f715;

    fn hashmix(value: u32, hash_constant: &mut u32) -> u32 {
        let mut value = value ^ *hash_constant;
        *hash_constant = hash_constant.wrapping_mul(MULT_A);
        value = value.wrapping_mul(*hash_constant);
        value ^ (value >> 16)
    }
    fn mix(left: u32, right: u32) -> u32 {
        let mut value = MIX_MULT_L
            .wrapping_mul(left)
            .wrapping_sub(MIX_MULT_R.wrapping_mul(right));
        value ^= value >> 16;
        value
    }

    let entropy = if seed > u32::MAX as u64 {
        vec![seed as u32, (seed >> 32) as u32]
    } else {
        vec![seed as u32]
    };
    let mut pool = [0u32; 4];
    let mut hash_constant = INIT_A;
    for index in 0..pool.len() {
        pool[index] = hashmix(entropy.get(index).copied().unwrap_or(0), &mut hash_constant);
    }
    for source in 0..pool.len() {
        for destination in 0..pool.len() {
            if source != destination {
                pool[destination] = mix(
                    pool[destination],
                    hashmix(pool[source], &mut hash_constant),
                );
            }
        }
    }
    for &word in entropy.iter().skip(pool.len()) {
        for destination in &mut pool {
            *destination = mix(*destination, hashmix(word, &mut hash_constant));
        }
    }

    // PCG64 requests four uint64 values = eight uint32 output words.
    let mut generated = [0u32; 8];
    let mut output_constant = INIT_B;
    for (index, value) in generated.iter_mut().enumerate() {
        let mut data = pool[index % pool.len()] ^ output_constant;
        output_constant = output_constant.wrapping_mul(MULT_B);
        data = data.wrapping_mul(output_constant);
        *value = data ^ (data >> 16);
    }
    [
        generated[0] as u64 | ((generated[1] as u64) << 32),
        generated[2] as u64 | ((generated[3] as u64) << 32),
        generated[4] as u64 | ((generated[5] as u64) << 32),
        generated[6] as u64 | ((generated[7] as u64) << 32),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numpy_choice_short_streams_match_numpy_1_26() {
        assert_eq!(
            numpy_choice_without_replacement(20, 8, 424_242).unwrap(),
            vec![13, 12, 1, 17, 14, 4, 5, 6],
        );
        assert_eq!(
            numpy_choice_without_replacement(20, 8, 0).unwrap(),
            vec![11, 1, 19, 7, 5, 4, 0, 8],
        );
    }

    #[test]
    fn fps_starts_at_zero_and_preserves_first_argmax() {
        let positions = [0.0, 0.0, 0.0, 2.0, 0.0, 0.0, -2.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        assert_eq!(farthest_point_indices(&positions, 3).unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn embedded_rows_have_released_width() {
        let row = [0.25, -0.5, 0.75, 1.0, 0.0, -1.0];
        for kind in [SkinTokensConditionKind::SkinVae, SkinTokensConditionKind::Michelangelo] {
            let embedded = embed_condition_rows(&row, kind).unwrap();
            assert_eq!(embedded.len(), SKIN_TOKENS_EMBEDDED_CONDITION_CHANNELS);
            assert_eq!(&embedded[51..], &row[3..]);
        }
    }
}
