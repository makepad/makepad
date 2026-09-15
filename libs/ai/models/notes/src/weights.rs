//! ONNX checkpoint census and tensor mapping.

use makepad_ai_loader::formats::onnx::{OnnxAttribute, OnnxGraph, OnnxModel};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct ConvWeights {
    pub out_channels: usize,
    pub in_channels: usize,
    pub kernel_time: usize,
    pub kernel_freq: usize,
    /// ONNX NCHW order: `[out, in, time, frequency]`.
    pub values: Vec<f32>,
    pub bias: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct CqtWeights {
    pub real: Vec<f32>,
    pub imag: Vec<f32>,
    pub downsample: Vec<f32>,
    pub normalization: Vec<f32>,
    pub bias: Vec<f32>,
    pub input_bn_scale: f32,
    pub input_bn_bias: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightCensus {
    pub initializer_count: usize,
    pub node_count: usize,
    pub op_counts: BTreeMap<String, usize>,
    pub network_parameter_count: usize,
    pub published_parameter_count: usize,
}

#[derive(Clone, Debug)]
pub struct NotesWeights {
    pub cqt: CqtWeights,
    pub contour: ConvWeights,
    pub contour_out: ConvWeights,
    pub note: ConvWeights,
    pub note_out: ConvWeights,
    pub onset: ConvWeights,
    pub onset_out: ConvWeights,
    pub census: WeightCensus,
}

const CQT_REAL: &str = "const_fold_opt__655";
const CQT_IMAG: &str = "const_fold_opt__664";
const CQT_DOWNSAMPLE: &str = "const_fold_opt__734";
const CQT_NORM: &str = "model_1/cq_t2010v2_1/Sqrt;model_1/cq_t2010v2_1/Sqrt";
const CQT_BIAS: &str = "model_1/cq_t2010v2_1/conv1d_25;model_1/cq_t2010v2_1/conv1d_25";
const INPUT_BN_SCALE: &str = "model_1/batch_normalization/FusedBatchNormV3;model_1/batch_normalization/FusedBatchNormV3";
const INPUT_BN_BIAS: &str = "model_1/batch_normalization/FusedBatchNormV3;model_1/batch_normalization/FusedBatchNormV31";

const CONTOUR_W: &str = "const_fold_opt__727";
const CONTOUR_B: &str = "model_1/re_lu_1/Relu;model_1/re_lu_1/Relu;model_1/batch_normalization_2/FusedBatchNormV3;model_1/batch_normalization_2/FusedBatchNormV3;model_1/conv2d_1/BiasAdd/ReadVariableOp;model_1/conv2d_1/BiasAdd/ReadVariableOp;model_1/conv2d_1/BiasAdd;model_1/conv2d_1/BiasAdd;model_1/conv2d_1/Conv2D;model_1/conv2d_1/Conv2D";
const CONTOUR_OUT_W: &str = "const_fold_opt__710";
const CONTOUR_OUT_B: &str = "model_1/contours-reduced/BiasAdd/ReadVariableOp;model_1/contours-reduced/BiasAdd/ReadVariableOp";
const NOTE_W: &str = "const_fold_opt__738";
const NOTE_B: &str = "model_1/conv2d_2/BiasAdd/ReadVariableOp;model_1/conv2d_2/BiasAdd/ReadVariableOp";
const NOTE_OUT_W: &str = "const_fold_opt__702";
const NOTE_OUT_B: &str = "model_1/conv2d_3/BiasAdd/ReadVariableOp;model_1/conv2d_3/BiasAdd/ReadVariableOp";
const ONSET_W: &str = "const_fold_opt__707";
const ONSET_B: &str = "model_1/re_lu_3/Relu;model_1/re_lu_3/Relu;model_1/batch_normalization_3/FusedBatchNormV3;model_1/batch_normalization_3/FusedBatchNormV3;model_1/conv2d_4/BiasAdd/ReadVariableOp;model_1/conv2d_4/BiasAdd/ReadVariableOp;model_1/conv2d_4/BiasAdd;model_1/conv2d_4/BiasAdd;model_1/conv2d_2/Conv2D;model_1/conv2d_2/Conv2D;model_1/conv2d_4/Conv2D;model_1/conv2d_4/Conv2D";
const ONSET_OUT_W: &str = "const_fold_opt__680";
const ONSET_OUT_B: &str = "model_1/conv2d_5/BiasAdd/ReadVariableOp;model_1/conv2d_5/BiasAdd/ReadVariableOp";

impl NotesWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let model = OnnxModel::load(path)?;
        validate_graph(&model.graph)?;
        let g = &model.graph;
        let cqt = CqtWeights {
            real: tensor(g, CQT_REAL, &[36, 1, 1, 256])?,
            imag: tensor(g, CQT_IMAG, &[36, 1, 1, 256])?,
            downsample: tensor(g, CQT_DOWNSAMPLE, &[1, 1, 1, 256])?,
            normalization: tensor(g, CQT_NORM, &[309, 1, 1])?,
            bias: tensor(g, CQT_BIAS, &[36])?,
            input_bn_scale: scalar(g, INPUT_BN_SCALE)?,
            input_bn_bias: scalar(g, INPUT_BN_BIAS)?,
        };
        let contour = conv(g, CONTOUR_W, CONTOUR_B, [8, 8, 3, 39])?;
        let contour_out = conv(g, CONTOUR_OUT_W, CONTOUR_OUT_B, [1, 8, 5, 5])?;
        let note = conv(g, NOTE_W, NOTE_B, [32, 1, 7, 7])?;
        let note_out = conv(g, NOTE_OUT_W, NOTE_OUT_B, [1, 32, 7, 3])?;
        let onset = conv(g, ONSET_W, ONSET_B, [32, 8, 5, 5])?;
        let onset_out = conv(g, ONSET_OUT_W, ONSET_OUT_B, [1, 33, 3, 3])?;
        let network_parameter_count = [
            &contour,
            &contour_out,
            &note,
            &note_out,
            &onset,
            &onset_out,
        ]
        .iter()
        .map(|layer| layer.values.len() + layer.bias.len())
        .sum();
        let mut op_counts = BTreeMap::new();
        for node in &g.nodes {
            *op_counts.entry(node.op_type.clone()).or_insert(0) += 1;
        }
        let census = WeightCensus {
            initializer_count: g.initializers.len(),
            node_count: g.nodes.len(),
            op_counts,
            network_parameter_count,
            // Keras includes gamma+beta for the three BatchNorm layers;
            // tf2onnx folds those 82 trainable scalars into conv weights/biases.
            published_parameter_count: network_parameter_count + 2 * (1 + 8 + 32),
        };
        Ok(Self {
            cqt,
            contour,
            contour_out,
            note,
            note_out,
            onset,
            onset_out,
            census,
        })
    }
}

fn validate_graph(graph: &OnnxGraph) -> Result<(), String> {
    if graph.inputs != ["serving_default_input_2:0"] {
        return Err(format!("Basic Pitch ONNX input changed: {:?}", graph.inputs));
    }
    if graph.outputs
        != [
            "StatefulPartitionedCall:2",
            "StatefulPartitionedCall:1",
            "StatefulPartitionedCall:0",
        ]
    {
        return Err(format!("Basic Pitch ONNX outputs changed: {:?}", graph.outputs));
    }
    if graph.initializers.len() != 102 || graph.nodes.len() != 248 {
        return Err(format!(
            "Basic Pitch ONNX census changed: {} initializers, {} nodes",
            graph.initializers.len(),
            graph.nodes.len()
        ));
    }
    let expected = [
        (CONTOUR_W, [3, 39].as_slice(), [1, 1].as_slice(), [1, 19, 1, 19].as_slice()),
        (CONTOUR_OUT_W, [5, 5].as_slice(), [1, 1].as_slice(), [2, 2, 2, 2].as_slice()),
        (NOTE_W, [7, 7].as_slice(), [1, 3].as_slice(), [3, 2, 3, 2].as_slice()),
        (NOTE_OUT_W, [7, 3].as_slice(), [1, 1].as_slice(), [3, 1, 3, 1].as_slice()),
        (ONSET_W, [5, 5].as_slice(), [1, 3].as_slice(), [2, 1, 2, 1].as_slice()),
        (ONSET_OUT_W, [3, 3].as_slice(), [1, 1].as_slice(), [1, 1, 1, 1].as_slice()),
    ];
    for (weight, kernel, stride, pads) in expected {
        let found = graph.nodes.iter().any(|node| {
            node.op_type == "Conv"
                && node.inputs.iter().any(|input| input == weight)
                &&
                matches!(node.attributes.get("kernel_shape"), Some(OnnxAttribute::Ints(v)) if v == kernel)
                && matches!(node.attributes.get("strides"), Some(OnnxAttribute::Ints(v)) if v == stride)
                && matches!(node.attributes.get("pads"), Some(OnnxAttribute::Ints(v)) if v == pads)
        });
        if !found {
            return Err(format!(
                "Basic Pitch ONNX is missing Conv weight={weight:?} kernel={kernel:?} stride={stride:?} pads={pads:?}"
            ));
        }
    }
    Ok(())
}

fn tensor(graph: &OnnxGraph, name: &str, shape: &[i64]) -> Result<Vec<f32>, String> {
    let value = graph
        .initializers
        .get(name)
        .ok_or_else(|| format!("Basic Pitch ONNX missing initializer '{name}'"))?;
    if value.dims != shape {
        return Err(format!(
            "Basic Pitch initializer '{name}' shape {:?}, expected {shape:?}",
            value.dims
        ));
    }
    value.f32_values()
}

fn scalar(graph: &OnnxGraph, name: &str) -> Result<f32, String> {
    let values = tensor(graph, name, &[1])?;
    Ok(values[0])
}

fn conv(
    graph: &OnnxGraph,
    weight: &str,
    bias: &str,
    shape: [usize; 4],
) -> Result<ConvWeights, String> {
    let dims: Vec<i64> = shape.iter().map(|&v| v as i64).collect();
    Ok(ConvWeights {
        out_channels: shape[0],
        in_channels: shape[1],
        kernel_time: shape[2],
        kernel_freq: shape[3],
        values: tensor(graph, weight, &dims)?,
        bias: tensor(graph, bias, &[shape[0] as i64])?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../local/models/weights/basic_pitch/nmp.onnx")
    }

    #[test]
    fn official_checkpoint_census_and_shapes() {
        let weights = NotesWeights::load(checkpoint()).expect("seeded Basic Pitch checkpoint");
        assert_eq!(weights.census.initializer_count, 102);
        assert_eq!(weights.census.node_count, 248);
        assert_eq!(weights.census.network_parameter_count, 16_700);
        assert_eq!(weights.census.published_parameter_count, 16_782);
        let expected_ops = BTreeMap::from([
            ("Add".to_string(), 2),
            ("Cast".to_string(), 3),
            ("Concat".to_string(), 20),
            ("Conv".to_string(), 32),
            ("Div".to_string(), 1),
            ("Equal".to_string(), 1),
            ("Log".to_string(), 1),
            ("Mul".to_string(), 6),
            ("Neg".to_string(), 9),
            ("Pad".to_string(), 24),
            ("ReduceMax".to_string(), 1),
            ("ReduceMin".to_string(), 1),
            ("ReduceSum".to_string(), 1),
            ("Relu".to_string(), 3),
            ("Reshape".to_string(), 67),
            ("Shape".to_string(), 1),
            ("Sigmoid".to_string(), 3),
            ("Slice".to_string(), 11),
            ("Sqrt".to_string(), 1),
            ("Sub".to_string(), 1),
            ("Transpose".to_string(), 21),
            ("Unsqueeze".to_string(), 37),
            ("Where".to_string(), 1),
        ]);
        assert_eq!(weights.census.op_counts, expected_ops);
        assert_eq!(weights.contour.values.len(), 8 * 8 * 3 * 39);
        assert_eq!(weights.onset_out.values.len(), 33 * 3 * 3);
    }
}
