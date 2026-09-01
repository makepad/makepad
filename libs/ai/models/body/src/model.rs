//! The whole model, end to end: one RGB image and a person box in, the pose
//! packet out. Crop -> backbone -> ray-conditioned context -> the six
//! decoder steps, each closing the loop through the pose head, the rig and
//! the camera -> the final rig parameters, keypoints and projection.
//!
//! Body decoder only (the reference's `inference_type = "body"`): the hand
//! parameters come from the body decoder, the hand crops of the reference's
//! "full" mode are a later phase.

use crate::backend::GpuTensor;
use crate::condition::{apply_mask_prompt, dense_pe_at, ray_features, RayCond};
use crate::decoder::{Decoder, LoopTiming, StepFeedback, StepInput};
use crate::dino::BodyDino;
use crate::hands::{hand_crops, BodyImage, FusionReport, HandBranch, HandOutput};
use crate::mask::{warp_mask, MaskEmbed};
use crate::mhr::MhrRig;
use crate::packet::{BodyPacket, BodyPerson, PersonHands};
use crate::pose::{camera_translation, model_params, project, unpack_pose};
use crate::preprocess::{
    condition_info, crop_geometry_at, crop_normalized, full_to_crop, patch_rays, CropGeometry,
};
use crate::weights::BodyWeights;
use crate::{
    DiffusionError, ProgressHook, Result, DINO_DIM, IMAGE_SIZE, MHR_JOINTS, NUM_KEYPOINTS, PATCH,
};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

pub struct BodyModel {
    pub weights: BodyWeights,
    pub(crate) dino: BodyDino,
    ray_cond: RayCond,
    gaussian: Vec<f32>,
    dense_pe: HashMap<usize, Vec<f32>>,
    pub(crate) no_mask_embed: [f32; DINO_DIM],
    pub(crate) decoder: Decoder,
    pub(crate) rig: MhrRig,
    hands: Option<HandBranch>,
    /// The mask-prompt encoder, loaded on the first masked call.
    mask_embed: Option<MaskEmbed>,
    /// The crop side the backbone sees; 512 is the trained size, smaller is
    /// faster and less accurate (see `set_crop_size`).
    crop_size: usize,
    /// Pose correctives on every refinement step (the reference) or only
    /// on the last one: the intermediate steps only feed keypoints back into
    /// the decoder, where the corrective's few millimetres barely register.
    pub correctives_every_step: bool,
    /// Per-stage wall times of the last `infer`, milliseconds:
    /// crop, backbone, context, decoder loop (incl. rig), packet.
    pub last_stage_ms: [f32; 5],
    /// The decoder loop's breakdown for the last `infer`.
    pub last_loop: LoopTiming,
}

/// What [`BodyModel::infer_full`] returns: the fused body, the two hand
/// passes (left first, already un-mirrored) and the fusion report.
#[derive(Clone, Debug)]
pub struct FullOutput {
    pub body: BodyOutput,
    pub hands: [HandOutput; 2],
    pub report: FusionReport,
}

impl BodyOutput {
    /// The transport subset of this output (spec section 9).
    pub fn into_person(self) -> BodyPerson {
        BodyPerson {
            mhr: self.mhr_model_params,
            global_rot: self.global_rot,
            cam_t: self.pred_cam_t,
            shape: self.shape,
            expr: self.face,
            focal: self.focal_length,
            bbox: self.bbox,
            kp3d: self.pred_keypoints_3d,
            kp2d: self.pred_keypoints_2d,
            joints: Some(self.pred_joint_coords),
            rots: None,
            hands: None,
        }
    }
}

impl FullOutput {
    /// The transport packet of a full-mode pass: the fused body plus which
    /// hands were trusted and where they were.
    pub fn into_packet(self, ms: f32) -> BodyPacket {
        let boxes = [self.hands[0].bbox, self.hands[1].bbox];
        let mut person = self.body.into_person();
        person.hands = Some(PersonHands {
            fused: self.report.fused,
            boxes,
        });
        BodyPacket {
            people: vec![person],
            ms,
        }
    }
}

/// Rich model output used by the body, hand, re-prompt, and fusion passes.
/// The packet API deliberately exposes only its stable transport subset.
#[derive(Clone, Debug)]
pub struct BodyOutput {
    pub pred_pose_raw: Vec<f32>,
    pub global_rot: [f32; 3],
    pub body_pose: [f32; 133],
    pub shape: [f32; 45],
    pub scale: [f32; 28],
    pub hand: [f32; 108],
    pub face: [f32; 72],
    pub pred_keypoints_3d: Vec<f32>,
    pub pred_vertices: Vec<f32>,
    pub pred_joint_coords: Vec<f32>,
    pub joint_global_rots: Vec<f32>,
    pub mhr_model_params: [f32; 204],
    pub pred_cam: [f32; 3],
    pub pred_keypoints_2d: Vec<f32>,
    pub pred_cam_t: [f32; 3],
    pub focal_length: f32,
    pub pred_keypoints_2d_depth: Vec<f32>,
    pub pred_keypoints_2d_cropped: Vec<f32>,
    pub hand_box: [[f32; 4]; 2],
    pub hand_logits: [[f32; 2]; 2],
    pub bbox: [f32; 4],
}

impl BodyModel {
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with_progress(path, None)
    }

    pub fn load_with_progress(path: &Path, progress: Option<ProgressHook>) -> Result<Self> {
        let weights = BodyWeights::load(path)?;
        let dino = BodyDino::prepare_with_progress(&weights, progress)?;
        let ray_cond = RayCond::prepare(&weights)?;
        let gaussian = weights.f32_shaped(
            "prompt_encoder.pe_layer.positional_encoding_gaussian_matrix",
            &[2, DINO_DIM / 2],
        )?;
        let no_mask = weights.f32_shaped("prompt_encoder.no_mask_embed.weight", &[1, DINO_DIM])?;
        let mut no_mask_embed = [0.0f32; DINO_DIM];
        no_mask_embed.copy_from_slice(&no_mask);
        let decoder = Decoder::load(&weights)?;
        let mut rig = MhrRig::load(&weights)?;
        rig.prepare_gpu()?;
        let mut model = Self {
            weights,
            dino,
            ray_cond,
            gaussian,
            dense_pe: HashMap::new(),
            no_mask_embed,
            decoder,
            rig,
            hands: None,
            mask_embed: None,
            crop_size: IMAGE_SIZE,
            correctives_every_step: false,
            last_stage_ms: [0.0; 5],
            last_loop: LoopTiming::default(),
        };
        model.set_crop_size(IMAGE_SIZE)?;
        Ok(model)
    }

    /// The crop side: a multiple of 16 between 128 and 1024. The model was
    /// trained at 512; 256 runs the backbone on a quarter of the tokens.
    /// The MHR rig, for callers that want the unposed mesh of a person the
    /// model just inferred (`rig().rest_vertices(&shape, &expr)`).
    pub fn rig(&self) -> &MhrRig {
        &self.rig
    }

    pub fn set_crop_size(&mut self, size: usize) -> Result<()> {
        if size % PATCH != 0 || !(128..=1024).contains(&size) {
            return Err(DiffusionError::workflow(format!(
                "body crop size {size}: must be a multiple of {PATCH} in 128..=1024"
            )));
        }
        let side = size / PATCH;
        if !self.dense_pe.contains_key(&size) {
            self.dense_pe.insert(size, dense_pe_at(&self.gaussian, side));
        }
        self.crop_size = size;
        Ok(())
    }

    pub fn crop_size(&self) -> usize {
        self.crop_size
    }

    /// FP8 backbone (E4M3 weights, tensor-core FP8 GEMM): about half the
    /// backbone time on Ada, at a measured accuracy cost (see the oracle test).
    pub fn set_backbone_fp8(&self, on: bool) {
        self.dino.set_fp8(on);
    }

    /// `rgb` is `width * height * 3` bytes; `bbox` is the person box in
    /// full-image pixels (xyxy), the whole image when `None`.
    pub fn infer(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        bbox: Option<[f32; 4]>,
    ) -> Result<BodyPacket> {
        self.infer_with(rgb, width, height, bbox, None)
    }

    /// [`Self::infer`] for one provided person box, optionally conditioned
    /// by that person's full-frame 0/1 segmentation mask and its confidence
    /// (the reference's mask prompt; spec 13.2).
    pub fn infer_masked(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        bbox: [f32; 4],
        mask: Option<(&[u8], f32)>,
    ) -> Result<BodyPacket> {
        self.infer_with(rgb, width, height, Some(bbox), mask)
    }

    fn infer_with(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        bbox: Option<[f32; 4]>,
        mask: Option<(&[u8], f32)>,
    ) -> Result<BodyPacket> {
        let total = Instant::now();
        let (last, _geo, _context) = self.body_pass(rgb, width, height, bbox, mask)?;
        let person = last.into_person();
        let t_packet = total.elapsed();
        self.last_stage_ms[4] = t_packet.as_secs_f32() * 1000.0 - self.last_stage_ms[..4].iter().sum::<f32>();
        Ok(BodyPacket {
            people: vec![person],
            ms: t_packet.as_secs_f32() * 1000.0,
        })
    }

    /// The body pass every entry point shares: crop (with the optional
    /// mask prompt), backbone, conditioning and the decoder loop. Returns
    /// the last step's output carrying the hand boxes, the crop geometry
    /// and the conditioned context (the hands pass re-prompts against it).
    fn body_pass(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        bbox: Option<[f32; 4]>,
        mask: Option<(&[u8], f32)>,
    ) -> Result<(BodyOutput, CropGeometry, GpuTensor)> {
        let (w, h) = (width as usize, height as usize);
        if rgb.len() != w * h * 3 {
            return Err(DiffusionError::workflow(format!(
                "body infer: {} bytes for {width}x{height} rgb, expected {}",
                rgb.len(),
                w * h * 3
            )));
        }
        if let Some((full_mask, _)) = mask {
            if full_mask.len() != w * h {
                return Err(DiffusionError::workflow(format!(
                    "body infer mask: {} bytes for {width}x{height}, expected {}",
                    full_mask.len(),
                    w * h
                )));
            }
            if self.crop_size != IMAGE_SIZE {
                return Err(DiffusionError::workflow(format!(
                    "body mask prompts require the {IMAGE_SIZE}px crop, current crop is {}",
                    self.crop_size
                )));
            }
        }
        let bbox = bbox.unwrap_or([0.0, 0.0, width as f32, height as f32]);
        let total = Instant::now();
        let geo = crop_geometry_at(bbox, w, h, None, self.crop_size, 1.25);
        let crop = crop_normalized(rgb, w, h, &geo);
        let crop_mask = mask.map(|(full_mask, _)| warp_mask(full_mask, w, h, &geo));
        let t_crop = total.elapsed();

        let embeddings = self.dino.forward_normalized(&crop)?;
        let t_backbone = total.elapsed();

        let embeddings = match (crop_mask, mask) {
            (Some(crop_mask), Some((_, score))) => {
                if self.mask_embed.is_none() {
                    self.mask_embed = Some(MaskEmbed::load(&self.weights)?);
                }
                let tokens = self.mask_embed.as_ref().unwrap().embed(&crop_mask);
                apply_mask_prompt(&embeddings, Some((&tokens, score)), &self.no_mask_embed)?
                    .unwrap_or(embeddings)
            }
            _ => embeddings,
        };
        let feats = ray_features(&patch_rays(&geo));
        let context = self.ray_cond.apply(&embeddings, &self.no_mask_embed, &feats)?;
        drop(embeddings);
        let t_context = total.elapsed();

        let tokens = self.decoder.build_tokens(condition_info(&geo));
        let rig = &self.rig;
        let dense_pe = &self.dense_pe[&self.crop_size];
        let every_step = self.correctives_every_step;
        let mut last: Option<BodyOutput> = None;
        let output = self
            .decoder
            .run(tokens, &context, dense_pe, |step: StepInput| {
                let correctives = every_step || step.layer + 1 == crate::DEC_DEPTH;
                let result = close_the_loop(rig, &geo, &step, correctives);
                let feedback = StepFeedback {
                    kp2d_cropped: result.pred_keypoints_2d_cropped.clone(),
                    depth: result.pred_keypoints_2d_depth.clone(),
                    kp3d: result.pred_keypoints_3d.clone(),
                };
                last = Some(result);
                feedback
            })?;
        self.last_loop = output.timing;
        let t_decoder = total.elapsed();

        let mut last = last.ok_or_else(|| DiffusionError::model("body decoder ran no steps"))?;
        last.hand_box = output.hand_boxes;
        last.hand_logits = output.hand_logits;
        last.bbox = bbox;
        self.last_stage_ms = [
            t_crop.as_secs_f32() * 1000.0,
            (t_backbone - t_crop).as_secs_f32() * 1000.0,
            (t_context - t_backbone).as_secs_f32() * 1000.0,
            (t_decoder - t_context).as_secs_f32() * 1000.0,
            0.0,
        ];
        Ok((last, geo, context))
    }

    /// Full body + two hand crops + wrist fusion. Unlike [`Self::infer`],
    /// this returns the rich fused result, the two hand passes and the
    /// fusion report (which hands were trusted and why). `mask` is the
    /// optional segmentation-mask prompt of the body pass (spec 13.2).
    pub fn infer_full(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        bbox: Option<[f32; 4]>,
        mask: Option<(&[u8], f32)>,
    ) -> Result<FullOutput> {
        if self.crop_size != IMAGE_SIZE {
            return Err(DiffusionError::workflow(format!(
                "body full inference requires the trained {IMAGE_SIZE}px crop, got {}",
                self.crop_size,
            )));
        }
        let (w, h) = (width as usize, height as usize);
        let (mut body, geo, context) = self.body_pass(rgb, width, height, bbox, mask)?;
        if self.hands.is_none() {
            self.hands = Some(HandBranch::load(&self.weights)?);
        }
        let branch = self.hands.as_ref().unwrap();
        let crops = hand_crops(
            &body,
            &geo,
            BodyImage {
                rgb,
                width: w,
                height: h,
            },
        );
        let left = branch.infer_hand(self, &crops[0])?;
        let right = branch.infer_hand(self, &crops[1])?;
        let report = branch.fuse(
            self,
            &mut body,
            &left,
            &right,
            &crops,
            &geo,
            &context,
            &self.dense_pe[&IMAGE_SIZE],
        )?;
        Ok(FullOutput {
            body,
            hands: [left, right],
            report,
        })
    }
}

/// One refinement step's tail: head output -> rig parameters -> posed rig
/// -> keypoints in camera axes -> camera translation -> projection.
pub(crate) fn close_the_loop(
    rig: &MhrRig,
    geo: &CropGeometry,
    step: &StepInput,
    correctives: bool,
) -> BodyOutput {
    let pose = unpack_pose(&step.pose_pred_519);
    let params = model_params(rig, &pose);
    let rigged = rig.forward(&pose.shape, &params, &pose.expr, correctives);
    // Rig output is centimetres in the rig's axes; the camera frame is
    // metres with y and z flipped.
    let to_camera = |values: &[f32], count: usize| -> Vec<f32> {
        let mut out = Vec::with_capacity(count * 3);
        for point in values[..count * 3].chunks_exact(3) {
            out.push(point[0] / 100.0);
            out.push(point[1] / -100.0);
            out.push(point[2] / -100.0);
        }
        out
    };
    let kp3d = to_camera(&rigged.keypoints308, NUM_KEYPOINTS);
    let vertices = to_camera(&rigged.verts, crate::MHR_VERTS);
    let mut joint_positions = Vec::with_capacity(MHR_JOINTS * 3);
    for joint in 0..MHR_JOINTS {
        joint_positions.extend_from_slice(&rigged.skel_state[joint * 8..joint * 8 + 3]);
    }
    let joints = to_camera(&joint_positions, MHR_JOINTS);
    let cam = [step.cam_pred_3[0], step.cam_pred_3[1], step.cam_pred_3[2]];
    let cam_t = camera_translation(cam, geo.center, geo.side, geo.focal, geo.principal);
    let (kp2d, depth) = project(&kp3d, cam_t, geo.focal, geo.principal);
    let pred_cam = cam;
    BodyOutput {
        pred_pose_raw: step.pose_pred_519[..266].to_vec(),
        global_rot: pose.global_rot,
        body_pose: pose.body,
        shape: pose.shape,
        scale: pose.scale,
        hand: pose.hands,
        face: pose.expr,
        pred_keypoints_3d: kp3d,
        pred_vertices: vertices,
        pred_joint_coords: joints,
        joint_global_rots: rigged.joint_global_rots,
        mhr_model_params: params,
        pred_cam,
        pred_keypoints_2d: kp2d.clone(),
        pred_cam_t: cam_t,
        focal_length: geo.focal,
        pred_keypoints_2d_depth: depth,
        pred_keypoints_2d_cropped: full_to_crop(&kp2d, geo),
        hand_box: [[0.0; 4]; 2],
        hand_logits: [[0.0; 2]; 2],
        bbox: [0.0; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::gpu_device_available;
    use crate::fixture;

    fn max_abs(actual: &[f32], expected: &[f32]) -> (f32, usize) {
        assert_eq!(actual.len(), expected.len());
        actual
            .iter()
            .zip(expected)
            .enumerate()
            .map(|(index, (a, b))| ((a - b).abs(), index))
            .fold((0.0, 0), |best, (error, index)| if error > best.0 { (error, index) } else { best })
    }

    /// The whole pipeline on the oracle's image against the reference's
    /// final outputs. The backbone runs in bf16 on both sides but not in the
    /// same order, so the answer differs by a few millimetres, not by zero.
    #[test]
    fn oracle_end_to_end() {
        let Some((shape, image)) = fixture::load("input_rgb_u8") else {
            eprintln!("SKIP oracle_end_to_end: fixtures absent");
            return;
        };
        let Some(weights_path) = fixture::weights_path() else {
            eprintln!("SKIP oracle_end_to_end: weights absent");
            return;
        };
        if !gpu_device_available() || !fixture::gpu_required_ops_available() {
            eprintln!("SKIP oracle_end_to_end: no GPU");
            return;
        }
        let (h, w) = (shape[0] as u32, shape[1] as u32);
        let rgb: Vec<u8> = image.iter().map(|v| *v as u8).collect();
        let load_started = Instant::now();
        let mut model = BodyModel::load(&weights_path).expect("load body model");
        eprintln!("body model load {:?}", load_started.elapsed());
        // The exact mode first (correctives on every step, as the reference).
        model.correctives_every_step = true;
        let packet = model.infer(&rgb, w, h, None).expect("infer");
        // A second run reports the warm timings.
        let packet = model.infer(&rgb, w, h, None).unwrap_or(packet);
        eprintln!(
            "body infer warm {:.1} ms: crop {:.1}, backbone {:.1}, context {:.1}, decoder+rig {:.1}, packet {:.1}; loop: layers {:.1}, heads {:.1}, rig+camera {:.1}, refine {:.1}",
            packet.ms,
            model.last_stage_ms[0],
            model.last_stage_ms[1],
            model.last_stage_ms[2],
            model.last_stage_ms[3],
            model.last_stage_ms[4],
            model.last_loop.layers_ms,
            model.last_loop.heads_ms,
            model.last_loop.step_ms,
            model.last_loop.refine_ms
        );
        // Correctives only on the final step: the cheaper loop, measured.
        model.correctives_every_step = false;
        let _ = model.infer(&rgb, w, h, None).expect("infer");
        let lean = model.infer(&rgb, w, h, None).expect("infer");
        let (lean3, _) = max_abs(&lean.people[0].kp3d, &fixture::load("final_pred_keypoints_3d").unwrap().1);
        eprintln!(
            "body correctives on the last step only: warm {:.1} ms (loop {:.1}); kp3d max {lean3:.4} m vs reference",
            lean.ms, model.last_stage_ms[3]
        );
        assert!(lean3 < 5.0e-3, "lean rig mode drifted: {lean3} m");
        // FP8 backbone: the tensor-core lever, measured.
        model.set_backbone_fp8(true);
        let _ = model.infer(&rgb, w, h, None).expect("infer fp8");
        let fp8 = model.infer(&rgb, w, h, None).expect("infer fp8");
        let (fp8_3, _) = max_abs(&fp8.people[0].kp3d, &fixture::load("final_pred_keypoints_3d").unwrap().1);
        let (fp8_2, _) = max_abs(&fp8.people[0].kp2d, &fixture::load("final_pred_keypoints_2d").unwrap().1);
        eprintln!(
            "body fp8 backbone: warm {:.1} ms (backbone {:.1}, loop {:.1}); kp3d max {fp8_3:.4} m, kp2d max {fp8_2:.2} px vs reference",
            fp8.ms, model.last_stage_ms[1], model.last_stage_ms[3]
        );
        model.set_backbone_fp8(false);
        // The same image through smaller crops: the speed/accuracy knob,
        // reported against the reference at 512 (not asserted tightly: these
        // sizes were never trained).
        let reference_kp3d = fixture::load("final_pred_keypoints_3d").unwrap().1;
        let reference_kp2d = fixture::load("final_pred_keypoints_2d").unwrap().1;
        for size in [384usize, 256, 192] {
            model.set_crop_size(size).expect("crop size");
            let _ = model.infer(&rgb, w, h, None).expect("infer at size");
            let small = model.infer(&rgb, w, h, None).expect("infer at size");
            let (e3, _) = max_abs(&small.people[0].kp3d, &reference_kp3d);
            let (e2, _) = max_abs(&small.people[0].kp2d, &reference_kp2d);
            let mean3: f32 = small.people[0].kp3d.iter().zip(&reference_kp3d).map(|(a, b)| (a - b).abs()).sum::<f32>() / reference_kp3d.len() as f32;
            eprintln!(
                "body crop {size}: warm {:.1} ms (backbone {:.1}, loop {:.1}); vs reference kp3d max {e3:.4} m mean {mean3:.4} m, kp2d max {e2:.2} px",
                small.ms, model.last_stage_ms[1], model.last_stage_ms[3]
            );
            assert!(e3 < 0.15, "crop {size} is off by {e3} m");
        }
        model.set_crop_size(IMAGE_SIZE).expect("crop size");
        let person = &packet.people[0];
        let (kp3d_err, kp3d_at) = max_abs(&person.kp3d, &fixture::load("final_pred_keypoints_3d").unwrap().1);
        let (kp2d_err, kp2d_at) = max_abs(&person.kp2d, &fixture::load("final_pred_keypoints_2d").unwrap().1);
        let (cam_err, _) = max_abs(&person.cam_t, &fixture::load("final_pred_cam_t").unwrap().1);
        let (rot_err, _) = max_abs(&person.global_rot, &fixture::load("final_global_rot").unwrap().1);
        let (mhr_err, mhr_at) = max_abs(&person.mhr, &fixture::load("final_mhr_model_params").unwrap().1);
        let (joint_err, _) = max_abs(
            person.joints.as_deref().unwrap(),
            &fixture::load("final_pred_joint_coords").unwrap().1,
        );
        eprintln!(
            "body end-to-end vs reference: kp3d {kp3d_err:.4} m (kp {}), kp2d {kp2d_err:.2} px (kp {}), cam_t {cam_err:.4}, global_rot {rot_err:.4}, rig params {mhr_err:.4} (at {mhr_at}), joints {joint_err:.4} m",
            kp3d_at / 3,
            kp2d_at / 2
        );
        assert!(kp3d_err < 2.0e-2, "kp3d max abs {kp3d_err} m");
        assert!(kp2d_err < 8.0, "kp2d max abs {kp2d_err} px");
        assert!(cam_err < 5.0e-2, "cam_t max abs {cam_err}");
        assert!(rot_err < 5.0e-2, "global_rot max abs {rot_err}");
        assert!(joint_err < 2.0e-2, "joints max abs {joint_err} m");
        let json = packet.to_json();
        assert!(json.starts_with("{\"n_people\":1,\"people\":[{\"mhr\":["));
        assert!(json.contains("\"kp3d\":[") && json.contains("\"joints\":["));
    }

    fn full_end_to_end(root: fixture::OracleRoot, label: &str) {
        let Some((shape, image)) = fixture::load_from(root, "input_rgb_u8") else {
            eprintln!("SKIP {label}: fixture absent");
            return;
        };
        if !gpu_device_available() || !fixture::gpu_required_ops_available() {
            eprintln!("SKIP {label}: no GPU");
            return;
        }
        let Some(weights_path) = fixture::weights_path() else {
            eprintln!("SKIP {label}: weights absent");
            return;
        };
        let (h, w) = (shape[0] as u32, shape[1] as u32);
        let rgb: Vec<u8> = image.into_iter().map(|value| value as u8).collect();
        let mut model = BodyModel::load(&weights_path).expect("load full body model");
        model.correctives_every_step = true;
        let full = model.infer_full(&rgb, w, h, None, None).expect("full inference");
        let field = |name: &str| fixture::load_from(root, name).unwrap().1;
        eprintln!("{label}: fusion report {:?}", full.report);
        let output = &full.body;
        // Parameter tolerances absorb the bf16 backbone noise of the hand
        // crops (2-3% relative), which the hand decoder amplifies more than
        // the body decoder does; the keypoint tolerances are the contract.
        let checks = [
            ("final_body_pose_params", output.body_pose.as_slice(), 2.0e-2),
            ("final_hand_pose_params", output.hand.as_slice(), 5.0e-2),
            ("final_scale_params", output.scale.as_slice(), 5.0e-2),
            ("final_shape_params", output.shape.as_slice(), 5.0e-2),
            ("final_pred_keypoints_3d", output.pred_keypoints_3d.as_slice(), 4.0e-3),
            ("final_pred_keypoints_2d", output.pred_keypoints_2d.as_slice(), 1.0),
        ];
        let mut failures = Vec::new();
        for (name, actual, tolerance) in checks {
            let expected = field(name);
            let (error, at) = max_abs(actual, &expected);
            eprintln!("{label}: {name} max abs {error:.6} at {at} (ours {} reference {})", actual[at], expected[at]);
            if error >= tolerance {
                failures.push(format!("{name} max abs {error} at {at}, tolerance {tolerance}"));
            }
        }
        assert!(failures.is_empty(), "{label}: {}", failures.join("; "));
    }

    #[test]
    fn oracle_full_end_to_end() {
        full_end_to_end(fixture::OracleRoot::Full, "oracle_full_end_to_end");
    }

    #[test]
    fn oracle_hands_end_to_end() {
        full_end_to_end(fixture::OracleRoot::Hands, "oracle_hands_end_to_end");
    }
}
