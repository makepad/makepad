use super::*;
use std::ops::Range;

/// A matrix group and optional half-open main-layer range. Later rules win.
/// Main-model groups never select MTP; `mtp` selects all its matrices.
#[derive(Clone, Debug)]
pub struct TypeRule {
    group: String,
    layers: Option<Range<usize>>,
    tensor_type: TensorType,
}

pub fn matrix_type(value: &str) -> Result<TensorType> {
    match value {
        "q4_k" => Ok(TensorType::Q4K),
        "q5_k" => Ok(TensorType::Q5K),
        "q6_k" => Ok(TensorType::Q6K),
        "q8_0" => Ok(TensorType::Q8_0),
        _ => Err(format!("unsupported matrix type {value}").into()),
    }
}

impl std::str::FromStr for TypeRule {
    type Err = Box<dyn std::error::Error + Send + Sync>;

    fn from_str(rule: &str) -> Result<Self> {
        let (selector, ty) = rule
            .split_once('=')
            .ok_or("expected GROUP[@START..END]=TYPE")?;
        let (group, layers) = if let Some((group, range)) = selector.split_once('@') {
            let (start, end) = range
                .split_once("..")
                .ok_or("expected START..END layer range")?;
            let range = start.parse::<usize>()?..end.parse::<usize>()?;
            if range.is_empty() {
                return Err("layer range must be nonempty and increasing".into());
            }
            (group, Some(range))
        } else {
            (selector, None)
        };
        if !matches!(
            group,
            "default"
                | "ffn"
                | "ffn_up"
                | "ffn_gate"
                | "ffn_down"
                | "attention"
                | "attn_q"
                | "attn_k"
                | "attn_v"
                | "attn_o"
                | "ssm"
                | "ssm_in"
                | "ssm_gate"
                | "ssm_alpha"
                | "ssm_beta"
                | "ssm_out"
                | "embedding"
                | "head"
                | "mtp"
        ) {
            return Err(format!("unknown matrix group {group}").into());
        }
        if layers.is_some() && matches!(group, "default" | "embedding" | "head" | "mtp") {
            return Err(format!("{group} does not accept a main-layer range").into());
        }
        Ok(Self {
            group: group.into(),
            layers,
            tensor_type: matrix_type(ty)?,
        })
    }
}

impl TypeRule {
    fn matches(&self, name: &str, layer: usize, main_layers: usize, tied: bool) -> bool {
        if self.layers.as_ref().is_some_and(|r| !r.contains(&layer)) {
            return false;
        }
        if self.group == "default" {
            return true;
        }
        if layer == main_layers {
            return self.group == "mtp";
        }
        let suffix = name.splitn(3, '.').nth(2).unwrap_or("");
        match self.group.as_str() {
            "embedding" => name == "token_embd.weight",
            "head" => name == "output.weight" || (tied && name == "token_embd.weight"),
            "ffn" => suffix.starts_with("ffn_"),
            "ffn_up" => suffix == "ffn_up.weight",
            "ffn_gate" => suffix == "ffn_gate.weight",
            "ffn_down" => suffix == "ffn_down.weight",
            "attention" => matches!(
                suffix,
                "attn_q.weight" | "attn_k.weight" | "attn_v.weight" | "attn_output.weight"
            ),
            "attn_q" => suffix == "attn_q.weight",
            "attn_k" => suffix == "attn_k.weight",
            "attn_v" => suffix == "attn_v.weight",
            "attn_o" => suffix == "attn_output.weight",
            "ssm" => matches!(
                suffix,
                "attn_qkv.weight"
                    | "attn_gate.weight"
                    | "ssm_alpha.weight"
                    | "ssm_beta.weight"
                    | "ssm_out.weight"
            ),
            "ssm_in" => suffix == "attn_qkv.weight",
            "ssm_gate" => suffix == "attn_gate.weight",
            "ssm_alpha" => suffix == "ssm_alpha.weight",
            "ssm_beta" => suffix == "ssm_beta.weight",
            "ssm_out" => suffix == "ssm_out.weight",
            _ => false,
        }
    }
}

pub(super) struct Rules {
    rules: Vec<TypeRule>,
    matched: Vec<bool>,
}

impl Rules {
    pub(super) fn new(options: &Options, layers: usize) -> Result<Self> {
        let mut rules = Vec::new();
        if let Some(preset) = &options.mix {
            if !matches!(preset.as_str(), "a" | "b" | "c" | "d") {
                return Err(format!("unknown mix {preset}; expected a, b, c or d").into());
            }
            for rule in [
                "default=q5_k",
                "ffn_up=q4_k",
                "ffn_gate=q4_k",
                "embedding=q6_k",
                "head=q6_k",
                "ssm_alpha=q8_0",
                "ssm_beta=q8_0",
            ] {
                rules.push(rule.parse()?);
            }
            if preset == "b" {
                rules.push(format!("ffn_down@{}..{}=q4_k", layers / 4, layers * 3 / 4).parse()?);
            }
            if matches!(preset.as_str(), "c" | "d") {
                rules.push("ffn_down=q4_k".parse()?);
            }
            if preset == "d" {
                rules.push("ssm_in=q6_k".parse()?);
                rules.push("ssm_out=q6_k".parse()?);
            }
        }
        rules.extend(options.rules.iter().cloned());
        for rule in &rules {
            if rule.layers.as_ref().is_some_and(|r| r.end > layers) {
                return Err(format!("rule layer range exceeds {layers}: {rule:?}").into());
            }
        }
        Ok(Self {
            matched: vec![false; rules.len()],
            rules,
        })
    }

    pub(super) fn select(
        &mut self,
        name: &str,
        layer: usize,
        layers: usize,
        tied: bool,
        mut ty: TensorType,
    ) -> TensorType {
        for (rule, matched) in self.rules.iter().zip(&mut self.matched) {
            if rule.matches(name, layer, layers, tied) {
                *matched = true;
                ty = rule.tensor_type;
            }
        }
        ty
    }

    pub(super) fn finish(&self) -> Result<()> {
        for (rule, matched) in self.rules.iter().zip(&self.matched) {
            if !matched {
                return Err(format!("rule matches no matrix: {rule:?}").into());
            }
        }
        Ok(())
    }
}

pub(super) fn file_type(tensors: &[Tensor]) -> u32 {
    let mut counts = BTreeMap::<u32, u64>::new();
    for tensor in tensors {
        let ty = tensor.spec.tensor_type;
        if ty != TensorType::F32 {
            *counts.entry(ty.ggml_type()).or_default() +=
                tensor.spec.dimensions.iter().product::<u64>();
        }
    }
    let dominant = counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(&ty, _)| ty);
    let mixed = counts.len() > 1;
    match dominant.and_then(TensorType::from_ggml_type) {
        Some(TensorType::F16) => 1,
        Some(TensorType::Q8_0) => 7,
        Some(TensorType::Q4K) => {
            if mixed {
                15
            } else {
                14
            }
        }
        Some(TensorType::Q5K) => {
            if mixed {
                17
            } else {
                16
            }
        }
        Some(TensorType::Q6K) => 18,
        _ => 0,
    }
}
