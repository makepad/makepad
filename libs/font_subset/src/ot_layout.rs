//! GSUB, GPOS, GDEF and `kern` rewritten to what touches the kept glyphs.
//!
//! Every lookup keeps its index (features, scripts and contextual lookup
//! records are unchanged); a lookup keeps only subtables that can apply to
//! kept glyphs, and each subtable only the entries whose glyphs are all kept
//! (input and output: a substitution whose result is gone never ran for the
//! text the subset draws). Class kerning keeps only the classes kept glyphs
//! use, mark attachment only the kept marks and bases and their classes.
//! GDEF's variation store keeps only the rows the kept device tables use.

use {
    crate::{
        graph::{Graph, Id, W},
        layout::{class_of, coverage},
        sfnt::{u16_at, u32_at},
        var_store, SubsetError,
    },
    std::collections::{BTreeMap, BTreeSet},
};

/// The device-table placeholder offset for "no variation".
const NO_VARIATION: (u16, u16) = (0xFFFF, 0xFFFF);

struct Ctx<'a> {
    d: &'a [u8],
    keep: &'a BTreeSet<u16>,
    g: Graph,
    /// The largest glyph id a kept substitution outputs.
    max_output: u16,
    /// VariationIndex device tables written: the object and its row.
    var_devices: BTreeMap<Id, (u16, u16)>,
}

impl<'a> Ctx<'a> {
    fn new(d: &'a [u8], keep: &'a BTreeSet<u16>, share: bool) -> Self {
        Self {
            d,
            keep,
            g: Graph::new(share),
            max_output: 0,
            var_devices: BTreeMap::new(),
        }
    }
    fn u16(&self, at: usize) -> Result<u16, SubsetError> {
        u16_at(self.d, at)
    }
    fn u32(&self, at: usize) -> Result<u32, SubsetError> {
        u32_at(self.d, at)
    }
    /// The 16-bit offset at `at`, measured from `base` (`None` when null).
    fn off(&self, base: usize, at: usize) -> Result<Option<usize>, SubsetError> {
        Ok(match self.u16(at)? {
            0 => None,
            o => Some(base + o as usize),
        })
    }
    fn kept(&self, g: u16) -> bool {
        self.keep.contains(&g)
    }
    fn output(&mut self, g: u16) {
        self.max_output = self.max_output.max(g);
    }

    /// Kept `(glyph, coverage index)` pairs of the coverage at `at`.
    fn covered(&self, at: usize) -> Result<Vec<(u16, u16)>, SubsetError> {
        Ok(coverage(self.d, at)?.into_iter().filter(|(g, _)| self.kept(*g)).collect())
    }

    fn coverage_obj(&mut self, glyphs: &[u16]) -> Id {
        let mut ranges: Vec<(u16, u16)> = Vec::new();
        for &g in glyphs {
            match ranges.last_mut() {
                Some((_, end)) if *end + 1 == g => *end = g,
                _ => ranges.push((g, g)),
            }
        }
        let mut w = W::new();
        if 6 * ranges.len() < 2 * glyphs.len() {
            w.u16(2).u16(ranges.len() as u16);
            let mut index = 0u16;
            for (start, end) in ranges {
                w.u16(start).u16(end).u16(index);
                index += end - start + 1;
            }
        } else {
            w.u16(1).u16(glyphs.len() as u16);
            for &g in glyphs {
                w.u16(g);
            }
        }
        self.g.add(w, 1)
    }

    /// The coverage at `at` cut to the kept glyphs (`None` when none is kept).
    fn kept_coverage(&mut self, at: usize) -> Result<Option<Id>, SubsetError> {
        let glyphs: Vec<u16> = self.covered(at)?.into_iter().map(|(g, _)| g).collect();
        Ok((!glyphs.is_empty()).then(|| self.coverage_obj(&glyphs)))
    }

    fn class_def_obj(&mut self, classes: &BTreeMap<u16, u16>) -> Id {
        let classes: Vec<(u16, u16)> = classes.iter().filter(|(_, c)| **c != 0).map(|(g, c)| (*g, *c)).collect();
        let mut ranges: Vec<(u16, u16, u16)> = Vec::new();
        for &(g, c) in &classes {
            match ranges.last_mut() {
                Some((_, end, class)) if *end + 1 == g && *class == c => *end = g,
                _ => ranges.push((g, g, c)),
            }
        }
        let mut w = W::new();
        let span = classes.last().map_or(0, |l| (l.0 - classes[0].0) as usize + 1);
        if classes.is_empty() || 4 + 6 * ranges.len() <= 6 + 2 * span {
            w.u16(2).u16(ranges.len() as u16);
            for (start, end, class) in ranges {
                w.u16(start).u16(end).u16(class);
            }
        } else {
            let start = classes[0].0;
            w.u16(1).u16(start).u16(span as u16);
            let mut values = vec![0u16; span];
            for (g, c) in classes {
                values[(g - start) as usize] = c;
            }
            for v in values {
                w.u16(v);
            }
        }
        self.g.add(w, 1)
    }

    /// The kept glyphs' classes in the class definition at `at` (null: all 0).
    fn kept_classes(&self, at: Option<usize>) -> Result<BTreeMap<u16, u16>, SubsetError> {
        let mut out = BTreeMap::new();
        for &g in self.keep {
            out.insert(g, if let Some(at) = at { class_of(self.d, at, g)? } else { 0 });
        }
        Ok(out)
    }

    fn device(&mut self, at: usize) -> Result<Id, SubsetError> {
        let (start, end, format) = (self.u16(at)?, self.u16(at + 2)?, self.u16(at + 4)?);
        let len = match format {
            1..=3 => {
                let count = end.saturating_sub(start) as usize + 1;
                let bits = 2usize << (format - 1);
                6 + 2 * (count * bits).div_ceil(16)
            }
            _ => 6,
        };
        let mut w = W::new();
        w.bytes(self.d.get(at..at + len).ok_or(SubsetError::Malformed("device table truncated"))?);
        let id = self.g.add(w, 1);
        if format == 0x8000 && (start, end) != NO_VARIATION {
            self.var_devices.insert(id, (start, end));
        }
        Ok(id)
    }

    fn anchor(&mut self, at: usize) -> Result<Id, SubsetError> {
        let format = self.u16(at)?;
        let mut w = W::new();
        match format {
            3 => {
                w.u16(3).u16(self.u16(at + 2)?).u16(self.u16(at + 4)?);
                for field in [at + 6, at + 8] {
                    let device = match self.off(at, field)? {
                        Some(d) => Some(self.device(d)?),
                        None => None,
                    };
                    w.off16(device);
                }
            }
            2 => {
                w.bytes(self.d.get(at..at + 8).ok_or(SubsetError::Malformed("anchor truncated"))?);
            }
            _ => {
                w.bytes(self.d.get(at..at + 6).ok_or(SubsetError::Malformed("anchor truncated"))?);
            }
        }
        Ok(self.g.add(w, 1))
    }

    fn anchor_off(&mut self, base: usize, at: usize) -> Result<Option<Id>, SubsetError> {
        match self.off(base, at)? {
            Some(a) => Ok(Some(self.anchor(a)?)),
            None => Ok(None),
        }
    }

    /// Copies a value record at `at` (its device offsets measured from
    /// `src_base`) into `w`, its devices measured from `dst_base` (`None`:
    /// the object `w` becomes). Returns the record's size.
    fn value(&mut self, w: &mut W, at: usize, format: u16, src_base: usize, dst_base: Option<Id>) -> Result<usize, SubsetError> {
        let mut read = at;
        for bit in 0..8 {
            if format & (1 << bit) == 0 {
                continue;
            }
            if bit < 4 {
                w.u16(self.u16(read)?);
            } else {
                let device = match self.off(src_base, read)? {
                    Some(d) => Some(self.device(d)?),
                    None => None,
                };
                match dst_base {
                    Some(base) => w.off16_from(device, base),
                    None => w.off16(device),
                };
            }
            read += 2;
        }
        Ok(read - at)
    }

    fn records(&self, at: usize, count: usize) -> Result<&'a [u8], SubsetError> {
        self.d.get(at..at + 4 * count).ok_or(SubsetError::Malformed("lookup records truncated"))
    }

    // GSUB.

    fn single_subst(&mut self, at: usize) -> Result<Option<Id>, SubsetError> {
        let format = self.u16(at)?;
        let mut pairs = Vec::new();
        for (g, i) in self.covered(at + self.u16(at + 2)? as usize)? {
            let out = match format {
                1 => g.wrapping_add(self.u16(at + 4)?),
                2 if i < self.u16(at + 4)? => self.u16(at + 6 + 2 * i as usize)?,
                _ => continue,
            };
            if self.kept(out) {
                pairs.push((g, out));
            }
        }
        if pairs.is_empty() {
            return Ok(None);
        }
        for &(_, out) in &pairs {
            self.output(out);
        }
        let glyphs: Vec<u16> = pairs.iter().map(|p| p.0).collect();
        let cov = self.coverage_obj(&glyphs);
        let delta = pairs[0].1.wrapping_sub(pairs[0].0);
        let mut w = W::new();
        if pairs.iter().all(|(g, out)| out.wrapping_sub(*g) == delta) {
            w.u16(1).off16(Some(cov)).u16(delta);
        } else {
            w.u16(2).off16(Some(cov)).u16(pairs.len() as u16);
            for (_, out) in pairs {
                w.u16(out);
            }
        }
        Ok(Some(self.g.add(w, 1)))
    }

    /// Multiple (2) and alternate (3) substitution: a glyph array per
    /// covered glyph.
    fn sequence_subst(&mut self, at: usize, alternate: bool) -> Result<Option<Id>, SubsetError> {
        let count = self.u16(at + 4)?;
        let mut kept = Vec::new();
        for (g, i) in self.covered(at + self.u16(at + 2)? as usize)? {
            if i >= count {
                continue;
            }
            let Some(seq) = self.off(at, at + 6 + 2 * i as usize)? else { continue };
            let n = self.u16(seq)? as usize;
            let mut glyphs = Vec::with_capacity(n);
            for k in 0..n {
                glyphs.push(self.u16(seq + 2 + 2 * k)?);
            }
            if alternate {
                // An alternate is chosen by index: one that is gone becomes
                // the glyph itself rather than shifting the others.
                for a in &mut glyphs {
                    if !self.kept(*a) {
                        *a = g;
                    }
                }
            } else if !glyphs.iter().all(|&o| self.kept(o)) {
                continue;
            }
            kept.push((g, glyphs));
        }
        if kept.is_empty() {
            return Ok(None);
        }
        let glyphs: Vec<u16> = kept.iter().map(|k| k.0).collect();
        let cov = self.coverage_obj(&glyphs);
        let mut w = W::new();
        w.u16(1).off16(Some(cov)).u16(kept.len() as u16);
        for (_, seq) in kept {
            let mut s = W::new();
            s.u16(seq.len() as u16);
            for o in seq {
                self.output(o);
                s.u16(o);
            }
            let id = self.g.add(s, 1);
            w.off16(Some(id));
        }
        Ok(Some(self.g.add(w, 1)))
    }

    fn ligature_subst(&mut self, at: usize) -> Result<Option<Id>, SubsetError> {
        let count = self.u16(at + 4)?;
        let mut kept = Vec::new();
        for (g, i) in self.covered(at + self.u16(at + 2)? as usize)? {
            if i >= count {
                continue;
            }
            let Some(set) = self.off(at, at + 6 + 2 * i as usize)? else { continue };
            let mut ligatures = Vec::new();
            for k in 0..self.u16(set)? as usize {
                let Some(lig) = self.off(set, set + 2 + 2 * k)? else { continue };
                let glyph = self.u16(lig)?;
                let components = self.u16(lig + 2)? as usize;
                let mut comps = Vec::new();
                for c in 1..components {
                    comps.push(self.u16(lig + 2 + 2 * c)?);
                }
                if self.kept(glyph) && comps.iter().all(|&c| self.kept(c)) {
                    ligatures.push((glyph, comps));
                }
            }
            if !ligatures.is_empty() {
                kept.push((g, ligatures));
            }
        }
        if kept.is_empty() {
            return Ok(None);
        }
        let glyphs: Vec<u16> = kept.iter().map(|k| k.0).collect();
        let cov = self.coverage_obj(&glyphs);
        let mut w = W::new();
        w.u16(1).off16(Some(cov)).u16(kept.len() as u16);
        for (_, ligatures) in kept {
            let mut set = W::new();
            set.u16(ligatures.len() as u16);
            for (glyph, comps) in ligatures {
                self.output(glyph);
                let mut l = W::new();
                l.u16(glyph).u16(comps.len() as u16 + 1);
                for c in comps {
                    l.u16(c);
                }
                let id = self.g.add(l, 1);
                set.off16(Some(id));
            }
            let id = self.g.add(set, 1);
            w.off16(Some(id));
        }
        Ok(Some(self.g.add(w, 1)))
    }

    fn reverse_subst(&mut self, at: usize) -> Result<Option<Id>, SubsetError> {
        let mut covs = Vec::new();
        let mut read = at + 4;
        for _ in 0..2 {
            let n = self.u16(read)? as usize;
            let mut list = Vec::new();
            for k in 0..n {
                let Some(c) = self.off(at, read + 2 + 2 * k)? else { return Ok(None) };
                match self.kept_coverage(c)? {
                    Some(id) => list.push(id),
                    None => return Ok(None),
                }
            }
            covs.push(list);
            read += 2 + 2 * n;
        }
        let glyph_count = self.u16(read)?;
        let mut pairs = Vec::new();
        for (g, i) in self.covered(at + self.u16(at + 2)? as usize)? {
            if i < glyph_count {
                let out = self.u16(read + 2 + 2 * i as usize)?;
                if self.kept(out) {
                    pairs.push((g, out));
                }
            }
        }
        if pairs.is_empty() {
            return Ok(None);
        }
        let glyphs: Vec<u16> = pairs.iter().map(|p| p.0).collect();
        let cov = self.coverage_obj(&glyphs);
        let mut w = W::new();
        w.u16(1).off16(Some(cov));
        for list in covs {
            w.u16(list.len() as u16);
            for id in list {
                w.off16(Some(id));
            }
        }
        w.u16(pairs.len() as u16);
        for (_, out) in pairs {
            self.output(out);
            w.u16(out);
        }
        Ok(Some(self.g.add(w, 1)))
    }

    // Contextual lookups (GSUB 5/6, GPOS 7/8).

    /// Reads `count` u16 values at `at`.
    fn array(&self, at: usize, count: usize) -> Result<Vec<u16>, SubsetError> {
        (0..count).map(|k| self.u16(at + 2 * k)).collect()
    }

    fn context(&mut self, at: usize, chained: bool) -> Result<Option<Id>, SubsetError> {
        match self.u16(at)? {
            1 => self.context_glyphs(at, chained),
            2 => self.context_classes(at, chained),
            3 => self.context_coverages(at, chained),
            _ => Ok(None),
        }
    }

    /// One rule: its sequences (chained: backtrack, input, lookahead; else
    /// input) with the input's first element implied, and its lookup records.
    fn read_rule(&self, at: usize, chained: bool) -> Result<(Vec<Vec<u16>>, &'a [u8]), SubsetError> {
        let mut read = at;
        let mut seqs = Vec::new();
        if chained {
            for part in 0..3 {
                let n = self.u16(read)? as usize;
                let len = if part == 1 { n.saturating_sub(1) } else { n };
                seqs.push(self.array(read + 2, len)?);
                read += 2 + 2 * len;
            }
            let records = self.u16(read)? as usize;
            Ok((seqs, self.records(read + 2, records)?))
        } else {
            let n = self.u16(read)? as usize;
            let records = self.u16(read + 2)? as usize;
            seqs.push(self.array(read + 4, n.saturating_sub(1))?);
            read += 4 + 2 * n.saturating_sub(1);
            Ok((seqs, self.records(read, records)?))
        }
    }

    fn write_rule(&mut self, seqs: &[Vec<u16>], records: &[u8], chained: bool) -> Id {
        let mut w = W::new();
        if chained {
            for (part, seq) in seqs.iter().enumerate() {
                w.u16(seq.len() as u16 + (part == 1) as u16);
                for &v in seq {
                    w.u16(v);
                }
            }
            w.u16((records.len() / 4) as u16).bytes(records);
        } else {
            w.u16(seqs[0].len() as u16 + 1).u16((records.len() / 4) as u16);
            for &v in &seqs[0] {
                w.u16(v);
            }
            w.bytes(records);
        }
        self.g.add(w, 1)
    }

    /// The rules of the rule set at `set` that `keep_rule` accepts.
    fn rule_set(&mut self, set: usize, chained: bool, keep_rule: &dyn Fn(&Self, &[Vec<u16>]) -> bool) -> Result<Option<Id>, SubsetError> {
        let mut rules = Vec::new();
        for k in 0..self.u16(set)? as usize {
            let Some(rule) = self.off(set, set + 2 + 2 * k)? else { continue };
            let (seqs, records) = self.read_rule(rule, chained)?;
            if keep_rule(self, &seqs) {
                rules.push(self.write_rule(&seqs, records, chained));
            }
        }
        if rules.is_empty() {
            return Ok(None);
        }
        let mut w = W::new();
        w.u16(rules.len() as u16);
        for r in rules {
            w.off16(Some(r));
        }
        Ok(Some(self.g.add(w, 1)))
    }

    fn context_glyphs(&mut self, at: usize, chained: bool) -> Result<Option<Id>, SubsetError> {
        let count = self.u16(at + 4)?;
        let mut kept = Vec::new();
        for (g, i) in self.covered(at + self.u16(at + 2)? as usize)? {
            if i >= count {
                continue;
            }
            let Some(set) = self.off(at, at + 6 + 2 * i as usize)? else { continue };
            let all_kept = |ctx: &Self, seqs: &[Vec<u16>]| seqs.iter().flatten().all(|&g| ctx.kept(g));
            if let Some(id) = self.rule_set(set, chained, &all_kept)? {
                kept.push((g, id));
            }
        }
        if kept.is_empty() {
            return Ok(None);
        }
        let glyphs: Vec<u16> = kept.iter().map(|k| k.0).collect();
        let cov = self.coverage_obj(&glyphs);
        let mut w = W::new();
        w.u16(1).off16(Some(cov)).u16(kept.len() as u16);
        for (_, id) in kept {
            w.off16(Some(id));
        }
        Ok(Some(self.g.add(w, 1)))
    }

    fn context_classes(&mut self, at: usize, chained: bool) -> Result<Option<Id>, SubsetError> {
        let glyphs: Vec<u16> = self.covered(at + self.u16(at + 2)? as usize)?.into_iter().map(|(g, _)| g).collect();
        if glyphs.is_empty() {
            return Ok(None);
        }
        // Class definitions: chained has backtrack, input, lookahead.
        let defs: Vec<Option<usize>> = if chained {
            vec![self.off(at, at + 4)?, self.off(at, at + 6)?, self.off(at, at + 8)?]
        } else {
            vec![self.off(at, at + 4)?]
        };
        let classes: Vec<BTreeMap<u16, u16>> = defs.iter().map(|d| self.kept_classes(*d)).collect::<Result<_, _>>()?;
        let present: Vec<BTreeSet<u16>> = classes.iter().map(|c| c.values().copied().collect()).collect();
        let input = if chained { 1 } else { 0 };
        let first_classes: BTreeSet<u16> = glyphs.iter().map(|g| classes[input][g]).collect();
        let sets_at = at + 4 + 2 * defs.len();
        let set_count = self.u16(sets_at)? as usize;
        let mut sets = Vec::with_capacity(set_count);
        let mut any = false;
        for c in 0..set_count {
            let set = match self.off(at, sets_at + 2 + 2 * c)? {
                Some(set) if first_classes.contains(&(c as u16)) => {
                    let present = present.clone();
                    let keep_rule = move |_: &Self, seqs: &[Vec<u16>]| {
                        seqs.iter().enumerate().all(|(part, seq)| {
                            let part = if chained { part } else { 0 };
                            seq.iter().all(|c| present[part].contains(c))
                        })
                    };
                    self.rule_set(set, chained, &keep_rule)?
                }
                _ => None,
            };
            any |= set.is_some();
            sets.push(set);
        }
        if !any {
            return Ok(None);
        }
        let cov = self.coverage_obj(&glyphs);
        let defs: Vec<Id> = classes.iter().map(|c| self.class_def_obj(c)).collect();
        let mut w = W::new();
        w.u16(2).off16(Some(cov));
        for d in defs {
            w.off16(Some(d));
        }
        w.u16(sets.len() as u16);
        for s in sets {
            w.off16(s);
        }
        Ok(Some(self.g.add(w, 1)))
    }

    fn context_coverages(&mut self, at: usize, chained: bool) -> Result<Option<Id>, SubsetError> {
        let mut w = W::new();
        w.u16(3);
        if chained {
            let mut read = at + 2;
            let mut parts = Vec::new();
            for _ in 0..3 {
                let n = self.u16(read)? as usize;
                let mut list = Vec::new();
                for k in 0..n {
                    let Some(c) = self.off(at, read + 2 + 2 * k)? else { return Ok(None) };
                    match self.kept_coverage(c)? {
                        Some(id) => list.push(id),
                        None => return Ok(None),
                    }
                }
                parts.push(list);
                read += 2 + 2 * n;
            }
            for list in parts {
                w.u16(list.len() as u16);
                for id in list {
                    w.off16(Some(id));
                }
            }
            let records = self.u16(read)? as usize;
            w.u16(records as u16).bytes(self.records(read + 2, records)?);
        } else {
            let n = self.u16(at + 2)? as usize;
            let records = self.u16(at + 4)? as usize;
            let mut list = Vec::new();
            for k in 0..n {
                let Some(c) = self.off(at, at + 6 + 2 * k)? else { return Ok(None) };
                match self.kept_coverage(c)? {
                    Some(id) => list.push(id),
                    None => return Ok(None),
                }
            }
            w.u16(n as u16).u16(records as u16);
            for id in list {
                w.off16(Some(id));
            }
            w.bytes(self.records(at + 6 + 2 * n, records)?);
        }
        Ok(Some(self.g.add(w, 1)))
    }

    // GPOS.

    fn single_pos(&mut self, at: usize) -> Result<Option<Id>, SubsetError> {
        let format = self.u16(at)?;
        let value_format = self.u16(at + 4)?;
        let covered = self.covered(at + self.u16(at + 2)? as usize)?;
        if covered.is_empty() {
            return Ok(None);
        }
        let glyphs: Vec<u16> = covered.iter().map(|c| c.0).collect();
        let cov = self.coverage_obj(&glyphs);
        let mut w = W::new();
        match format {
            1 => {
                w.u16(1).off16(Some(cov)).u16(value_format);
                self.value(&mut w, at + 6, value_format, at, None)?;
            }
            2 => {
                let size = 2 * (value_format & 0xFF).count_ones() as usize;
                w.u16(2).off16(Some(cov)).u16(value_format).u16(covered.len() as u16);
                for (_, i) in covered {
                    self.value(&mut w, at + 8 + size * i as usize, value_format, at, None)?;
                }
            }
            _ => return Ok(None),
        }
        Ok(Some(self.g.add(w, 1)))
    }

    fn pair_pos(&mut self, at: usize) -> Result<Option<Id>, SubsetError> {
        let format = self.u16(at)?;
        let (vf1, vf2) = (self.u16(at + 4)?, self.u16(at + 6)?);
        let size = 2 * ((vf1 & 0xFF).count_ones() + (vf2 & 0xFF).count_ones()) as usize;
        let covered = self.covered(at + self.u16(at + 2)? as usize)?;
        if covered.is_empty() {
            return Ok(None);
        }
        let id = self.g.reserve();
        let mut w = W::new();
        match format {
            1 => {
                let count = self.u16(at + 8)?;
                let mut sets = Vec::new();
                for (g, i) in covered {
                    if i >= count {
                        continue;
                    }
                    let Some(set) = self.off(at, at + 10 + 2 * i as usize)? else { continue };
                    let mut pairs = Vec::new();
                    for k in 0..self.u16(set)? as usize {
                        let record = set + 2 + k * (2 + size);
                        if self.kept(self.u16(record)?) {
                            pairs.push(record);
                        }
                    }
                    if !pairs.is_empty() {
                        let mut s = W::new();
                        s.u16(pairs.len() as u16);
                        for record in pairs {
                            s.u16(self.u16(record)?);
                            let read = self.value(&mut s, record + 2, vf1, at, Some(id))?;
                            self.value(&mut s, record + 2 + read, vf2, at, Some(id))?;
                        }
                        sets.push((g, self.g.add(s, 1)));
                    }
                }
                if sets.is_empty() {
                    self.g.set(id, W::new(), 1);
                    return Ok(None);
                }
                let glyphs: Vec<u16> = sets.iter().map(|s| s.0).collect();
                let cov = self.coverage_obj(&glyphs);
                w.u16(1).off16(Some(cov)).u16(vf1).u16(vf2).u16(sets.len() as u16);
                for (_, s) in sets {
                    w.off16(Some(s));
                }
            }
            2 => {
                let def1 = self.off(at, at + 8)?;
                let def2 = self.off(at, at + 10)?;
                let (count1, count2) = (self.u16(at + 12)? as usize, self.u16(at + 14)? as usize);
                let mut rows = BTreeSet::from([0u16]);
                let mut first_classes = BTreeMap::new();
                for &(g, _) in &covered {
                    let c = if let Some(d) = def1 { class_of(self.d, d, g)? } else { 0 };
                    rows.insert(c);
                    first_classes.insert(g, c);
                }
                let second_classes = self.kept_classes(def2)?;
                let columns: BTreeSet<u16> = second_classes.values().copied().chain([0]).collect();
                let rows: Vec<u16> = rows.into_iter().filter(|&r| (r as usize) < count1).collect();
                let columns: Vec<u16> = columns.into_iter().filter(|&c| (c as usize) < count2).collect();
                let renumber = |list: &[u16], classes: &BTreeMap<u16, u16>| -> BTreeMap<u16, u16> {
                    classes
                        .iter()
                        .filter_map(|(&g, c)| list.iter().position(|x| x == c).map(|p| (g, p as u16)))
                        .collect()
                };
                let new1 = renumber(&rows, &first_classes);
                let new2 = renumber(&columns, &second_classes);
                let glyphs: Vec<u16> = covered.iter().map(|c| c.0).collect();
                let cov = self.coverage_obj(&glyphs);
                let d1 = self.class_def_obj(&new1);
                let d2 = self.class_def_obj(&new2);
                w.u16(2).off16(Some(cov)).u16(vf1).u16(vf2).off16(Some(d1)).off16(Some(d2));
                w.u16(rows.len() as u16).u16(columns.len() as u16);
                for &r in &rows {
                    for &c in &columns {
                        let record = at + 16 + (r as usize * count2 + c as usize) * size;
                        let read = self.value(&mut w, record, vf1, at, None)?;
                        self.value(&mut w, record + read, vf2, at, None)?;
                    }
                }
            }
            _ => {
                self.g.set(id, W::new(), 1);
                return Ok(None);
            }
        }
        self.g.set(id, w, 1);
        Ok(Some(id))
    }

    fn cursive_pos(&mut self, at: usize) -> Result<Option<Id>, SubsetError> {
        let count = self.u16(at + 4)?;
        let covered: Vec<(u16, u16)> = self.covered(at + self.u16(at + 2)? as usize)?.into_iter().filter(|c| c.1 < count).collect();
        if covered.is_empty() {
            return Ok(None);
        }
        let glyphs: Vec<u16> = covered.iter().map(|c| c.0).collect();
        let cov = self.coverage_obj(&glyphs);
        let mut w = W::new();
        w.u16(1).off16(Some(cov)).u16(covered.len() as u16);
        for (_, i) in covered {
            let record = at + 6 + 4 * i as usize;
            let entry = self.anchor_off(at, record)?;
            let exit = self.anchor_off(at, record + 2)?;
            w.off16(entry).off16(exit);
        }
        Ok(Some(self.g.add(w, 1)))
    }

    /// Mark-to-base (4), mark-to-ligature (5), mark-to-mark (6).
    fn mark_pos(&mut self, at: usize, ligature: bool) -> Result<Option<Id>, SubsetError> {
        let marks = self.covered(at + self.u16(at + 2)? as usize)?;
        let bases = self.covered(at + self.u16(at + 4)? as usize)?;
        let class_count = self.u16(at + 6)? as usize;
        let mark_array = at + self.u16(at + 8)? as usize;
        let base_array = at + self.u16(at + 10)? as usize;
        if marks.is_empty() || bases.is_empty() {
            return Ok(None);
        }
        let mut classes = BTreeSet::new();
        for &(_, i) in &marks {
            classes.insert(self.u16(mark_array + 2 + 4 * i as usize)?);
        }
        let classes: Vec<u16> = classes.into_iter().filter(|&c| (c as usize) < class_count).collect();
        let mut ma = W::new();
        ma.u16(marks.len() as u16);
        let mut mark_glyphs = Vec::new();
        for &(g, i) in &marks {
            let record = mark_array + 2 + 4 * i as usize;
            let Some(class) = classes.iter().position(|&c| c == self.u16(record).unwrap_or(u16::MAX)) else {
                continue;
            };
            let anchor = self.anchor_off(mark_array, record + 2)?;
            ma.u16(class as u16).off16(anchor);
            mark_glyphs.push(g);
        }
        if mark_glyphs.len() != marks.len() {
            return Err(SubsetError::Malformed("mark class out of range"));
        }
        let mut ba = W::new();
        ba.u16(bases.len() as u16);
        for &(_, i) in &bases {
            if ligature {
                let Some(attach) = self.off(base_array, base_array + 2 + 2 * i as usize)? else {
                    ba.off16(None);
                    continue;
                };
                let components = self.u16(attach)? as usize;
                let mut la = W::new();
                la.u16(components as u16);
                for k in 0..components {
                    for &c in &classes {
                        let anchor = self.anchor_off(attach, attach + 2 + 2 * (k * class_count + c as usize))?;
                        la.off16(anchor);
                    }
                }
                let id = self.g.add(la, 1);
                ba.off16(Some(id));
            } else {
                for &c in &classes {
                    let anchor = self.anchor_off(base_array, base_array + 2 + 2 * (i as usize * class_count + c as usize))?;
                    ba.off16(anchor);
                }
            }
        }
        let mark_cov = self.coverage_obj(&mark_glyphs);
        let base_glyphs: Vec<u16> = bases.iter().map(|b| b.0).collect();
        let base_cov = self.coverage_obj(&base_glyphs);
        let ma = self.g.add(ma, 1);
        let ba = self.g.add(ba, 1);
        let mut w = W::new();
        w.u16(1).off16(Some(mark_cov)).off16(Some(base_cov)).u16(classes.len() as u16).off16(Some(ma)).off16(Some(ba));
        Ok(Some(self.g.add(w, 1)))
    }

    fn subtable(&mut self, gsub: bool, kind: u16, at: usize) -> Result<Option<Id>, SubsetError> {
        match (gsub, kind) {
            (true, 1) => self.single_subst(at),
            (true, 2) => self.sequence_subst(at, false),
            (true, 3) => self.sequence_subst(at, true),
            (true, 4) => self.ligature_subst(at),
            (true, 5) | (false, 7) => self.context(at, false),
            (true, 6) | (false, 8) => self.context(at, true),
            (true, 8) => self.reverse_subst(at),
            (false, 1) => self.single_pos(at),
            (false, 2) => self.pair_pos(at),
            (false, 3) => self.cursive_pos(at),
            (false, 4) | (false, 6) => self.mark_pos(at, false),
            (false, 5) => self.mark_pos(at, true),
            _ => Err(SubsetError::Unsupported("unknown lookup type")),
        }
    }

    // The table's lists.

    fn feature(&mut self, tag: [u8; 4], at: usize) -> Result<Id, SubsetError> {
        let params = match self.off(at, at)? {
            Some(p) => {
                let len = match &tag {
                    b"size" => Some(10),
                    [b's', b's', ..] => Some(4),
                    [b'c', b'v', ..] => Some(14 + 3 * self.u16(p + 12)? as usize),
                    _ => None,
                };
                match len.and_then(|len| self.d.get(p..p + len)) {
                    Some(bytes) => {
                        let mut w = W::new();
                        w.bytes(bytes);
                        Some(self.g.add(w, 0))
                    }
                    None => None,
                }
            }
            None => None,
        };
        let count = self.u16(at + 2)? as usize;
        let mut w = W::new();
        w.off16(params).u16(count as u16).bytes(self.d.get(at + 4..at + 4 + 2 * count).ok_or(SubsetError::Malformed("feature truncated"))?);
        Ok(self.g.add(w, 0))
    }

    fn lang_sys(&mut self, at: usize) -> Result<Id, SubsetError> {
        let count = self.u16(at + 4)? as usize;
        let mut w = W::new();
        w.bytes(self.d.get(at..at + 6 + 2 * count).ok_or(SubsetError::Malformed("language system truncated"))?);
        Ok(self.g.add(w, 0))
    }

    fn script_list(&mut self, at: usize) -> Result<Id, SubsetError> {
        let count = self.u16(at)? as usize;
        let mut w = W::new();
        w.u16(count as u16);
        for i in 0..count {
            let record = at + 2 + 6 * i;
            let script = at + self.u16(record + 4)? as usize;
            let default = match self.off(script, script)? {
                Some(l) => Some(self.lang_sys(l)?),
                None => None,
            };
            let langs = self.u16(script + 2)? as usize;
            let mut s = W::new();
            s.off16(default).u16(langs as u16);
            for k in 0..langs {
                let lang = script + 4 + 6 * k;
                let id = self.lang_sys(script + self.u16(lang + 4)? as usize)?;
                s.bytes(&self.d[lang..lang + 4]).off16(Some(id));
            }
            let id = self.g.add(s, 0);
            w.bytes(&self.d[record..record + 4]).off16(Some(id));
        }
        Ok(self.g.add(w, 0))
    }

    fn feature_list(&mut self, at: usize) -> Result<Id, SubsetError> {
        let count = self.u16(at)? as usize;
        let mut w = W::new();
        w.u16(count as u16);
        for i in 0..count {
            let record = at + 2 + 6 * i;
            let tag: [u8; 4] = self.d[record..record + 4].try_into().unwrap();
            let id = self.feature(tag, at + self.u16(record + 4)? as usize)?;
            w.bytes(&tag).off16(Some(id));
        }
        Ok(self.g.add(w, 0))
    }

    fn feature_variations(&mut self, at: usize, feature_list: usize) -> Result<Id, SubsetError> {
        let count = self.u32(at + 4)? as usize;
        let mut w = W::new();
        w.u32(self.u32(at)?).u32(count as u32);
        for i in 0..count {
            let record = at + 8 + 8 * i;
            let set = at + self.u32(record)? as usize;
            let conditions = self.u16(set)? as usize;
            let mut s = W::new();
            s.u16(conditions as u16);
            for k in 0..conditions {
                let condition = set + self.u32(set + 2 + 4 * k)? as usize;
                if self.u16(condition)? != 1 {
                    return Err(SubsetError::Unsupported("feature variation condition format"));
                }
                let mut c = W::new();
                c.bytes(&self.d[condition..condition + 8]);
                let id = self.g.add(c, 0);
                s.off32(Some(id));
            }
            let set_id = self.g.add(s, 0);
            let subst = at + self.u32(record + 4)? as usize;
            let substitutions = self.u16(subst + 4)? as usize;
            let mut t = W::new();
            t.u32(self.u32(subst)?).u16(substitutions as u16);
            for k in 0..substitutions {
                let r = subst + 6 + 6 * k;
                let index = self.u16(r)?;
                let tag_at = feature_list + 2 + 6 * index as usize;
                let tag: [u8; 4] = self.d.get(tag_at..tag_at + 4).ok_or(SubsetError::Malformed("feature index"))?.try_into().unwrap();
                let id = self.feature(tag, subst + self.u32(r + 2)? as usize)?;
                t.u16(index).off32(Some(id));
            }
            let subst_id = self.g.add(t, 0);
            w.off32(Some(set_id)).off32(Some(subst_id));
        }
        Ok(self.g.add(w, 0))
    }

    /// The whole GSUB (`gsub`) or GPOS table's graph; returns its root.
    fn layout(&mut self, gsub: bool, extension: bool) -> Result<Id, SubsetError> {
        let d = self.d;
        let minor = self.u16(2)?;
        let script_list = self.script_list(self.u16(4)? as usize)?;
        let feature_list_at = self.u16(6)? as usize;
        let feature_list = self.feature_list(feature_list_at)?;
        let lookup_list_at = self.u16(8)? as usize;
        let ext_kind = if gsub { 7 } else { 9 };
        let count = self.u16(lookup_list_at)? as usize;
        let mut ll = W::new();
        ll.u16(count as u16);
        for i in 0..count {
            let lookup = lookup_list_at + self.u16(lookup_list_at + 2 + 2 * i)? as usize;
            let mut kind = self.u16(lookup)?;
            let flag = self.u16(lookup + 2)?;
            let subtables = self.u16(lookup + 4)? as usize;
            let mut kept = Vec::new();
            for k in 0..subtables {
                let mut at = lookup + self.u16(lookup + 6 + 2 * k)? as usize;
                let mut sub_kind = kind;
                if kind == ext_kind {
                    sub_kind = self.u16(at + 2)?;
                    at += self.u32(at + 4)? as usize;
                }
                if let Some(id) = self.subtable(gsub, sub_kind, at)? {
                    kept.push(id);
                }
                if k + 1 == subtables {
                    kind = sub_kind;
                }
            }
            let mut l = W::new();
            l.u16(if extension { ext_kind } else { kind }).u16(flag).u16(kept.len() as u16);
            for id in kept {
                if extension {
                    let mut e = W::new();
                    e.u16(1).u16(kind).off32(Some(id));
                    let e = self.g.add(e, 0);
                    l.off16(Some(e));
                } else {
                    l.off16(Some(id));
                }
            }
            if flag & 0x10 != 0 {
                l.u16(self.u16(lookup + 6 + 2 * subtables)?);
            }
            let id = self.g.add(l, 0);
            ll.off16(Some(id));
        }
        let lookup_list = self.g.add(ll, 0);
        let mut w = W::new();
        w.u16(1).u16(minor.min(1)).off16(Some(script_list)).off16(Some(feature_list)).off16(Some(lookup_list));
        if minor >= 1 {
            let variations = match u32_at(d, 10)? {
                0 => None,
                o => Some(self.feature_variations(o as usize, feature_list_at)?),
            };
            w.off32(variations);
        }
        Ok(self.g.add(w, 0))
    }

    /// GDEF's graph, its variation store left for the caller (`store`).
    fn gdef(&mut self, store: Option<Id>) -> Result<Id, SubsetError> {
        let minor = self.u16(2)?;
        let glyph_classes = match self.off(0, 4)? {
            Some(c) => {
                let classes = self.kept_classes(Some(c))?;
                Some(self.class_def_obj(&classes))
            }
            None => None,
        };
        let attach = match self.off(0, 6)? {
            Some(a) => self.attach_list(a)?,
            None => None,
        };
        let carets = match self.off(0, 8)? {
            Some(c) => self.caret_list(c)?,
            None => None,
        };
        let mark_classes = match self.off(0, 10)? {
            Some(c) => {
                let classes = self.kept_classes(Some(c))?;
                Some(self.class_def_obj(&classes))
            }
            None => None,
        };
        let mut w = W::new();
        w.u16(1).u16(minor.min(3)).off16(glyph_classes).off16(attach).off16(carets).off16(mark_classes);
        if minor >= 2 {
            let sets = match self.off(0, 12)? {
                Some(s) => {
                    let count = self.u16(s + 2)? as usize;
                    let mut m = W::new();
                    m.u16(1).u16(count as u16);
                    for k in 0..count {
                        let cov = s + self.u32(s + 4 + 4 * k)? as usize;
                        let glyphs: Vec<u16> = self.covered(cov)?.into_iter().map(|c| c.0).collect();
                        let id = self.coverage_obj(&glyphs);
                        m.off32(Some(id));
                    }
                    Some(self.g.add(m, 0))
                }
                None => None,
            };
            w.off16(sets);
        }
        if minor >= 3 {
            w.off32(store);
        }
        Ok(self.g.add(w, 0))
    }

    fn attach_list(&mut self, at: usize) -> Result<Option<Id>, SubsetError> {
        let covered = self.covered(at + self.u16(at)? as usize)?;
        let count = self.u16(at + 2)?;
        let covered: Vec<(u16, u16)> = covered.into_iter().filter(|c| c.1 < count).collect();
        if covered.is_empty() {
            return Ok(None);
        }
        let glyphs: Vec<u16> = covered.iter().map(|c| c.0).collect();
        let cov = self.coverage_obj(&glyphs);
        let mut w = W::new();
        w.off16(Some(cov)).u16(covered.len() as u16);
        for (_, i) in covered {
            let point = at + self.u16(at + 4 + 2 * i as usize)? as usize;
            let n = self.u16(point)? as usize;
            let mut p = W::new();
            p.bytes(self.d.get(point..point + 2 + 2 * n).ok_or(SubsetError::Malformed("attach point truncated"))?);
            let id = self.g.add(p, 1);
            w.off16(Some(id));
        }
        Ok(Some(self.g.add(w, 0)))
    }

    fn caret_list(&mut self, at: usize) -> Result<Option<Id>, SubsetError> {
        let covered = self.covered(at + self.u16(at)? as usize)?;
        let count = self.u16(at + 2)?;
        let covered: Vec<(u16, u16)> = covered.into_iter().filter(|c| c.1 < count).collect();
        if covered.is_empty() {
            return Ok(None);
        }
        let glyphs: Vec<u16> = covered.iter().map(|c| c.0).collect();
        let cov = self.coverage_obj(&glyphs);
        let mut w = W::new();
        w.off16(Some(cov)).u16(covered.len() as u16);
        for (_, i) in covered {
            let lig = at + self.u16(at + 4 + 2 * i as usize)? as usize;
            let carets = self.u16(lig)? as usize;
            let mut l = W::new();
            l.u16(carets as u16);
            for k in 0..carets {
                let caret = lig + self.u16(lig + 2 + 2 * k)? as usize;
                let mut c = W::new();
                c.u16(self.u16(caret)?).u16(self.u16(caret + 2)?);
                if self.u16(caret)? == 3 {
                    let device = match self.off(caret, caret + 4)? {
                        Some(d) => Some(self.device(d)?),
                        None => None,
                    };
                    c.off16(device);
                }
                let id = self.g.add(c, 1);
                l.off16(Some(id));
            }
            let id = self.g.add(l, 1);
            w.off16(Some(id));
        }
        Ok(Some(self.g.add(w, 0)))
    }
}

/// The rewritten GSUB, GPOS and GDEF (each `None` when absent) and the
/// largest glyph id a kept substitution outputs.
pub(crate) struct Layout {
    pub gsub: Option<Vec<u8>>,
    pub gpos: Option<Vec<u8>>,
    pub gdef: Option<Vec<u8>>,
    pub max_output: u16,
}

pub(crate) fn prune(
    gsub: Option<&[u8]>,
    gpos: Option<&[u8]>,
    gdef: Option<&[u8]>,
    keep: &BTreeSet<u16>,
) -> Result<Layout, SubsetError> {
    // Layouts to try, until the offsets fit: compact, then every subtable
    // behind an extension lookup, then without sharing equal objects.
    const ATTEMPTS: [(bool, bool); 3] = [(false, true), (true, true), (true, false)];
    let mut out = Layout {
        gsub: None,
        gpos: None,
        gdef: None,
        max_output: 0,
    };
    if let Some(d) = gsub {
        let mut result = Err(SubsetError::Overflow);
        for (extension, share) in ATTEMPTS {
            let mut ctx = Ctx::new(d, keep, share);
            let root = ctx.layout(true, extension)?;
            result = ctx.g.pack(root).map(|bytes| (bytes, ctx.max_output));
            if !matches!(result, Err(SubsetError::Overflow)) {
                break;
            }
        }
        let (bytes, max_output) = result?;
        out.gsub = Some(bytes);
        out.max_output = max_output;
    }
    if gpos.is_none() && gdef.is_none() {
        return Ok(out);
    }
    for (extension, share) in ATTEMPTS {
        let mut pos = gpos.map(|d| Ctx::new(d, keep, share));
        let pos_root = match &mut pos {
            Some(ctx) => Some(ctx.layout(false, extension)?),
            None => None,
        };
        let mut def = gdef.map(|d| Ctx::new(d, keep, share));
        let mut packed_def = None;
        if let Some(ctx) = &mut def {
            // GPOS devices and GDEF carets share GDEF's variation store: it
            // keeps the rows they use, renumbered.
            let store_at = if ctx.u16(2)? >= 3 { ctx.u32(14)? as usize } else { 0 };
            let mut store = None;
            let mut remap = Default::default();
            if store_at != 0 {
                // Carets are written before the store is known: build GDEF
                // once to collect them, then the store, then patch.
                let mut used: BTreeSet<(u16, u16)> = BTreeSet::new();
                let mut probe = Ctx::new(ctx.d, keep, share);
                probe.gdef(None)?;
                used.extend(probe.var_devices.values().copied());
                if let Some(p) = &pos {
                    used.extend(p.var_devices.values().copied());
                }
                let (id, map) = var_store::subset(ctx.d, store_at, &used, &mut ctx.g)?;
                store = id;
                remap = map;
            }
            let root = ctx.gdef(store)?;
            let patch = |ctx: &mut Ctx, remap: &BTreeMap<(u16, u16), (u16, u16)>| {
                let devices: Vec<(Id, (u16, u16))> = ctx.var_devices.iter().map(|(k, v)| (*k, *v)).collect();
                for (id, row) in devices {
                    if let Some(&(outer, inner)) = remap.get(&row) {
                        let data = ctx.g.data_mut(id);
                        data[..2].copy_from_slice(&outer.to_be_bytes());
                        data[2..4].copy_from_slice(&inner.to_be_bytes());
                    }
                }
            };
            if store_at != 0 {
                patch(ctx, &remap);
                if let Some(p) = &mut pos {
                    patch(p, &remap);
                }
            }
            packed_def = Some(ctx.g.pack(root));
        }
        let packed_pos = match (&pos, pos_root) {
            (Some(ctx), Some(root)) => Some(ctx.g.pack(root)),
            _ => None,
        };
        let overflow = matches!(packed_def, Some(Err(SubsetError::Overflow))) || matches!(packed_pos, Some(Err(SubsetError::Overflow)));
        if overflow && (extension, share) != ATTEMPTS[2] {
            continue;
        }
        out.gpos = packed_pos.transpose()?;
        out.gdef = packed_def.transpose()?;
        break;
    }
    Ok(out)
}

/// The `kern` table with only the kept pairs in its format 0 subtables
/// (other formats and Apple's version are copied).
pub(crate) fn prune_kern(kern: &[u8], keep: &BTreeSet<u16>) -> Result<Vec<u8>, SubsetError> {
    if u16_at(kern, 0)? != 0 {
        return Ok(kern.to_vec());
    }
    let count = u16_at(kern, 2)? as usize;
    let mut out = Vec::new();
    out.extend_from_slice(&kern[..4]);
    let mut at = 4;
    for _ in 0..count {
        let length = u16_at(kern, at + 2)? as usize;
        let coverage = u16_at(kern, at + 4)?;
        if coverage >> 8 == 0 {
            let pairs = u16_at(kern, at + 6)? as usize;
            let mut kept = Vec::new();
            for k in 0..pairs {
                let p = at + 14 + 6 * k;
                let record = kern.get(p..p + 6).ok_or(SubsetError::Malformed("kern pairs truncated"))?;
                if keep.contains(&u16_at(record, 0)?) && keep.contains(&u16_at(record, 2)?) {
                    kept.extend_from_slice(record);
                }
            }
            let n = kept.len() / 6;
            let mut selector = 0u16;
            while (2usize << selector) <= n {
                selector += 1;
            }
            let range = if n == 0 { 0 } else { 6u16 << selector };
            out.extend_from_slice(&u16_at(kern, at)?.to_be_bytes());
            out.extend_from_slice(&((14 + kept.len()) as u16).to_be_bytes());
            out.extend_from_slice(&coverage.to_be_bytes());
            out.extend_from_slice(&(n as u16).to_be_bytes());
            out.extend_from_slice(&range.to_be_bytes());
            out.extend_from_slice(&(if n == 0 { 0 } else { selector }).to_be_bytes());
            out.extend_from_slice(&((n as u16 * 6).saturating_sub(range)).to_be_bytes());
            out.extend_from_slice(&kept);
            // Large format 0 tables overflow the 16-bit length; the pair
            // count is what tells their size.
            at += 14 + 6 * pairs;
        } else {
            out.extend_from_slice(kern.get(at..at + length).ok_or(SubsetError::Malformed("kern subtable truncated"))?);
            at += length;
        }
    }
    Ok(out)
}
