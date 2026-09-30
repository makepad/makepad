//! Kernel-driven uniforms (PDOOM-PARITY AK11): a kernel's reduce result,
//! or words of one element's record, become a pass's or a material's
//! uniform values for the frame (a lens ring's radius from the glyphs'
//! extent, a camera's follow target from the descent's head, landing
//! times). A host reads its feeds from the job that produced them (the
//! realtime stream's latest, or locked time's synchronous run) and writes
//! them into the pass values or draw uniforms by name.

use makepad_script_compute::kernel::Layout;
use makepad_script_compute::sched::Job;

/// Where one uniform component comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum FeedSource {
    /// Lane `lane` of the job's reduce result.
    Reduced { lane: usize },
    /// Word `word` of record `element` of output buffer `buffer`, read as
    /// an f32.
    Word { buffer: String, element: usize, stride: usize, word: usize },
}

/// A uniform fed by a kernel: up to four components.
#[derive(Clone, Debug, PartialEq)]
pub struct Feed {
    pub uniform: String,
    pub sources: Vec<FeedSource>,
}

impl Feed {
    /// `width` reduce lanes from `first`.
    pub fn reduced(uniform: &str, first: usize, width: usize) -> Feed {
        Feed { uniform: uniform.into(), sources: (first..first + width.clamp(1, 4)).map(|lane| FeedSource::Reduced { lane }).collect() }
    }

    /// Field `field` of record `element` of `buffer`, written in `layout`.
    pub fn field(uniform: &str, buffer: &str, element: usize, layout: &Layout, field: &str) -> Result<Feed, String> {
        let f = layout.fields.iter().find(|f| f.name == field).ok_or_else(|| {
            let names: Vec<&str> = layout.fields.iter().map(|f| f.name.as_str()).collect();
            format!("`{}` has no field `{}` (it has {})", layout.name, field, names.join(", "))
        })?;
        let width = (f.ty.words() as usize).clamp(1, 4);
        Ok(Feed {
            uniform: uniform.into(),
            sources: (0..width).map(|k| FeedSource::Word { buffer: buffer.into(), element, stride: layout.stride as usize, word: f.offset as usize + k }).collect(),
        })
    }

    /// The uniform's value from a finished job (components it cannot read
    /// stay 0: an element past the output, a lane the kernel lacks).
    pub fn read(&self, job: &Job) -> [f32; 4] {
        let mut v = [0.0f32; 4];
        for (k, s) in self.sources.iter().take(4).enumerate() {
            v[k] = match s {
                FeedSource::Reduced { lane } => job.stats().reduced.get(*lane).copied().unwrap_or(0.0),
                FeedSource::Word { buffer, element, stride, word } => job.out_u32(buffer).and_then(|w| w.get(element * stride + word)).map_or(0.0, |w| f32::from_bits(*w)),
            };
        }
        v
    }
}

/// Writes every feed's value into `values`, the uniforms declared as
/// `names` (a pass's values in declaration order); feeds naming no declared
/// uniform are returned.
pub fn apply<'a>(feeds: &'a [Feed], job: &Job, names: &[&str], values: &mut [[f32; 4]]) -> Vec<&'a str> {
    let mut unknown = Vec::new();
    for f in feeds {
        match names.iter().position(|n| *n == f.uniform) {
            Some(k) if k < values.len() => values[k] = f.read(job),
            _ => unknown.push(f.uniform.as_str()),
        }
    }
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::kernel::{FieldTy, LayoutField};
    use crate::compute::sched::InlineExecutor;

    #[test]
    fn reduce_results_and_records_feed_uniforms() {
        // The extent of some points (reduce) and the head of a trail
        // (element 0's record) as a pass's `ring` and `follow` uniforms.
        let pts: Vec<u32> = (0..100).map(|i| ((i as f32 * 0.37).sin() * 5.0).to_bits()).collect();
        let k = crate::compile("let p = input(f32)\nfn reduce_max(i) { vec2(abs(p[i]), float(i)) }", &[], &[]).unwrap();
        let mut job = Job::new(k, 100);
        job.input_vec_u32("p", pts.clone()).unwrap();
        job.run(&InlineExecutor, 1).unwrap();
        let head = Layout { name: "Head".into(), stride: 4, fields: vec![LayoutField { name: "pos".into(), ty: FieldTy::Vec3, offset: 0 }, LayoutField { name: "age".into(), ty: FieldTy::F32, offset: 3 }] };
        let k2 = crate::compile("let h = output(Head)\nfn element(i) { h[i].pos = vec3(float(i), 2.0, 3.0)\n h[i].age = 0.5 }", &[head.clone()], &[]).unwrap();
        let mut job2 = Job::new(k2, 3);
        job2.output_u32("h", vec![0; 12]).unwrap();
        job2.run(&InlineExecutor, 1).unwrap();
        let mut values = [[9.0f32; 4]; 3];
        let want = pts.iter().map(|w| f32::from_bits(*w).abs()).fold(0.0f32, f32::max);
        assert!(apply(&[Feed::reduced("ring", 0, 1)], &job, &["amount", "ring", "follow"], &mut values).is_empty());
        assert_eq!(values[1], [want, 0.0, 0.0, 0.0]);
        let follow = Feed::field("follow", "h", 2, &head, "pos").unwrap();
        assert_eq!(apply(&[follow, Feed::reduced("nope", 0, 1)], &job2, &["amount", "ring", "follow"], &mut values), vec!["nope"]);
        assert_eq!(values[2], [2.0, 2.0, 3.0, 0.0]);
        assert_eq!(values[0], [9.0; 4], "untouched");
        assert!(Feed::field("x", "h", 0, &head, "speed").unwrap_err().contains("has no field `speed` (it has pos, age)"));
    }
}
