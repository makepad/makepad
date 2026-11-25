//! A nested rate path uses one half-band stage for each absolute doubling.
//! Keep the two filter delays exact until the highest boundary is known.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct RatePath {
    pub(super) absolute: u32,
    // Unpadded samples at this path's current top rate, per direction.
    filters: [u32; 2],
    // Linear and hold phase offsets, in base frames.
    phase: f64,
}

impl RatePath {
    pub(super) fn base(rate: u32) -> Self {
        Self { absolute: rate, filters: [0; 2], phase: 0.0 }
    }

    pub(super) fn extend(self, factor: u32, modes: RateModes, has_input: bool) -> Self {
        let absolute = self.absolute * factor;
        let mut stage = self.absolute;
        let mut delay = 0;
        while stage < absolute {
            stage *= 2;
            // Group delays of the 67-, 19- and 15-tap stages, in stage samples.
            delay = 2 * delay + match stage { 2 => 33, 4 => 9, 8 => 7, _ => unreachable!() };
        }
        let mut filters = self.filters.map(|n| n * factor);
        let last = f64::from(factor - 1) / f64::from(absolute);
        let mut phase = self.phase;
        if has_input {
            match modes.up {
                RateMode::Filter => filters[0] += delay,
                RateMode::Linear => phase += last,
                RateMode::Hold => phase += last * 0.5,
            }
        }
        match modes.down {
            RateMode::Filter => filters[1] += delay,
            RateMode::Linear => phase -= last * 0.5,
            RateMode::Hold => phase -= last,
        }
        Self { absolute, filters, phase }
    }

    pub(super) fn pads(self) -> [u32; 2] {
        self.filters.map(|n| (self.absolute - n % self.absolute) % self.absolute)
    }

    pub(super) fn latency(self) -> u32 {
        let [up, down] = self.filters.map(|n| n.div_ceil(self.absolute));
        // Nearest base frame, with half-frame ties going to the earlier frame.
        (f64::from(up + down) + self.phase - 0.5).ceil().max(0.0) as u32
    }
}

struct Region {
    site: usize,
    path: RatePath,
    deepest: u32,
    required: u32,
}

#[derive(Default)]
pub(super) struct RateNests {
    stack: Vec<Region>,
    // Inlined sites in encounter order. A branch encloses another region.
    pub(super) observed: Vec<bool>,
    pub(super) branches: Vec<bool>,
}

impl RateNests {
    pub(super) fn plan(&mut self, parent_rate: u32, factor: u32, modes: RateModes, has_input: bool, span: Span)
        -> LResult<(usize, RatePath, [u32; 2])> {
        let absolute = parent_rate * factor;
        if absolute > 8 {
            return err(span, format!("nested oversample reaches {absolute}×; the largest supported absolute rate is 8×"));
        }
        let parent = self.stack.last().map_or(RatePath::base(parent_rate), |r| r.path);
        let path = parent.extend(factor, modes, has_input);
        let site = self.observed.len();
        self.observed.push(false);
        let pads = if self.branches.get(site) == Some(&true) { [0; 2] } else { path.pads() };
        Ok((site, path, pads))
    }

    pub(super) fn push(&mut self, site: usize, path: RatePath) {
        if let Some(parent) = self.stack.last() {
            self.observed[parent.site] = true;
        }
        self.stack.push(Region { site, path, deepest: path.absolute, required: 0 });
    }

    pub(super) fn finish(&mut self, factor: u32, modes: RateModes, declared: u32, span: Span) -> LResult<()> {
        let region = self.stack.pop().unwrap();
        let branch = self.observed[region.site];
        let latency = if branch { region.required } else { region.path.latency() };
        if let Some(parent) = self.stack.last_mut() {
            parent.required = parent.required.max(latency);
            parent.deepest = parent.deepest.max(region.deepest);
            return Ok(());
        }
        if declared >= latency { return Ok(()); }
        if branch || region.path.absolute > factor {
            return err(span, format!("the oversample nest up to {}× needs {latency} base frames of latency", region.deepest));
        }
        let d = halfband_down_latency(factor);
        let (path, suggestion) = if latency == 2 * d {
            ("round-trip", format!("HALFBAND_LATENCY_{factor}"))
        } else {
            let path = if modes.down == RateMode::Filter { "down" } else { "up" };
            let adjustment = match latency.cmp(&d) {
                std::cmp::Ordering::Less => " - 1",
                std::cmp::Ordering::Equal => "",
                std::cmp::Ordering::Greater => " + 1",
            };
            (path, format!("HALFBAND_DOWN_LATENCY_{factor}{adjustment}"))
        };
        err(span, format!("the {factor}x oversample region needs {latency} frames of {path} latency; declare latency({suggestion})"))
    }
}

impl Lowerer<'_> {
    // The state describes the local number of stages. Their tap tables
    // depend on where those stages sit in the absolute cascade.
    pub(super) fn region_filter(&mut self, state: Place, input: V, destination: Option<&Place>,
        parent: u32, factor: u32, pad: u32, span: Span) -> LResult<V> {
        let direction = if destination.is_some() { "up" } else { "down" };
        let mut args = vec![V::Place(state), input];
        let name = if parent == 1 && pad == (if factor == 8 { 3 } else { 1 }) {
            if let Some(buffer) = destination.filter(|_| factor == 8) { args.push(V::Place(buffer.clone())); }
            format!("halfband_{direction}{factor}")
        } else {
            for stage in [2, 4].into_iter().take(factor.ilog2() as usize) {
                let name = format!("HALFBAND_TAPS_{}", parent * stage);
                let Some(Bind::Place(table)) = self.globals.get(&name) else {
                    return err(span, format!("oversample needs the prelude table `{name}`"));
                };
                args.push(V::Place(table.clone()));
            }
            args.push(V::Lit(f64::from(pad)));
            match (factor, destination.is_some()) {
                (2, true) => "halfband_up_pad".into(),
                (2, false) => "halfband_down".into(),
                (4, _) => format!("halfband_{direction}4_stages"),
                _ => unreachable!("8x already uses every stage"),
            }
        };
        let result = self.halfband_call(&name, args, span)?;
        if let Some(buffer) = destination.filter(|_| factor != 8) { self.write(buffer, &result, span)?; }
        Ok(result)
    }

    // A cheap conversion at the top can still owe pad from a filtered
    // ancestor. Its otherwise unused filter ring holds the previous samples.
    pub(super) fn pad_region_buffer(&mut self, state: &Place, buffer: &Place,
        factor: u32, pad: u32, span: Span) -> LResult<()> {
        let mut first = state.clone();
        for _ in 1..factor.ilog2() { first = self.field_place(&first, "low", span)?; }
        let history = self.field_place(&first, "ring", span)?;
        if !matches!(&history.ty, T::Array(t, n) if **t == T::F && *n >= pad) {
            return err(span, format!("oversample needs at least {pad} samples of filter history for the nest pad"));
        }
        let mut samples = Vec::new();
        for (place, count) in [(&history, pad), (buffer, factor)] {
            for i in 0..count {
                let (base, extent, off) = self.offset_plus(place, i);
                samples.push(self.b.load(Ty::F32, place.region, base, extent, off));
            }
        }
        for i in 0..factor { self.write_word(buffer, i, samples[i as usize]); }
        for i in 0..pad { self.write_word(&history, i, samples[(factor + i) as usize]); }
        Ok(())
    }
}
