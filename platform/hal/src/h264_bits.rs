//! H.264 parameter sets written by us (ITU-T H.264 7.3.2.1 / 7.3.2.2 / E.1.1): the SPS and PPS
//! a hardware stream encoder's slices go with. Hardware encode APIs code slices only; the
//! parameter sets are the stream's contract with every decoder, so they are written here from
//! one description ([`StreamParams`]) that also configures the encoder.
//!
//! The VUI always carries `bitstream_restriction` with `max_num_reorder_frames = 0` and
//! `max_dec_frame_buffering = 1`: without it a decoder may hold frames back for reordering
//! (VideoToolbox does), which is a frame or more of latency on a live stream.

/// A big-endian bit writer for RBSP syntax (u(n), ue(v), se(v)).
#[derive(Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    bits: u32,
    acc: u8,
}

impl BitWriter {
    pub fn bit(&mut self, b: bool) {
        self.acc = (self.acc << 1) | b as u8;
        self.bits += 1;
        if self.bits == 8 {
            self.bytes.push(self.acc);
            self.acc = 0;
            self.bits = 0;
        }
    }

    /// u(n): `n` <= 32 bits of `v`, most significant first.
    pub fn u(&mut self, n: u32, v: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1 != 0);
        }
    }

    /// ue(v): unsigned Exp-Golomb.
    pub fn ue(&mut self, v: u32) {
        let x = v as u64 + 1;
        let len = 64 - x.leading_zeros();
        for _ in 0..len - 1 {
            self.bit(false);
        }
        for i in (0..len).rev() {
            self.bit((x >> i) & 1 != 0);
        }
    }

    /// se(v): signed Exp-Golomb (9.1.1: k > 0 -> 2k - 1, k <= 0 -> -2k).
    pub fn se(&mut self, v: i32) {
        let k = v as i64;
        self.ue(if k > 0 { (2 * k - 1) as u32 } else { (-2 * k) as u32 });
    }

    /// rbsp_trailing_bits(): a stop bit, then zeros to the byte boundary.
    pub fn trailing(&mut self) {
        self.bit(true);
        while self.bits != 0 {
            self.bit(false);
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        debug_assert_eq!(self.bits, 0, "RBSP not byte aligned");
        self.bytes
    }
}

/// A NAL unit with a 4-byte start code: header byte, then the RBSP with emulation prevention
/// (7.4.1: 0x03 after two zero bytes that a byte <= 3 follows).
pub fn nal(nal_ref_idc: u8, nal_unit_type: u8, rbsp: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&[0, 0, 0, 1, (nal_ref_idc & 3) << 5 | (nal_unit_type & 31)]);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
}

pub const NAL_SLICE: u8 = 1;
pub const NAL_IDR: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;

/// Everything the parameter sets say about a low-latency stream: 4:2:0 or 4:4:4 8-bit, progressive,
/// P frames referencing one previous frame, POC type 2 (output order = decode order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamParams {
    /// 66 baseline, 77 main, 100 high, 244 high 4:4:4 predictive.
    pub profile_idc: u8,
    /// 1: 4:2:0, 3: 4:4:4 (high profiles only write it).
    pub chroma_format_idc: u8,
    /// level_idc (51 = 5.1).
    pub level_idc: u8,
    /// The picture, in pixels (even sizes); the coded size is this rounded up to macroblocks,
    /// the rest cropped.
    pub width: u32,
    pub height: u32,
    /// log2_max_frame_num_minus4.
    pub log2_max_frame_num_minus4: u8,
    pub max_num_ref_frames: u8,
    /// CABAC (main/high) instead of CAVLC.
    pub cabac: bool,
    /// transform_8x8_mode_flag (high only).
    pub transform_8x8: bool,
    pub pic_init_qp_minus26: i8,
    pub chroma_qp_index_offset: i8,
    /// video_full_range_flag; colour description 1/1/1 (BT.709) is always written.
    pub full_range: bool,
}

impl StreamParams {
    pub fn mbs_w(&self) -> u32 {
        self.width.div_ceil(16)
    }
    pub fn mbs_h(&self) -> u32 {
        self.height.div_ceil(16)
    }
    /// frame_crop_right_offset / frame_crop_bottom_offset in crop units (4:2:0: 2 pixels each
    /// way; 4:4:4: 1 pixel).
    pub fn crop(&self) -> (u32, u32) {
        let unit = if self.chroma_format_idc == 3 { 1 } else { 2 };
        ((self.mbs_w() * 16 - self.width) / unit, (self.mbs_h() * 16 - self.height) / unit)
    }

    /// seq_parameter_set_rbsp() (id 0).
    pub fn sps_rbsp(&self) -> Vec<u8> {
        let mut w = BitWriter::default();
        w.u(8, self.profile_idc as u32);
        // constraint_set1 for baseline/main streams a constrained baseline decoder takes; none
        // for high. reserved_zero_2bits follow.
        let constraints = match self.profile_idc {
            66 => 0b0100_0000,
            77 => 0b0100_0000,
            _ => 0,
        };
        w.u(8, constraints);
        w.u(8, self.level_idc as u32);
        w.ue(0); // seq_parameter_set_id
        if matches!(self.profile_idc, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
            w.ue(self.chroma_format_idc as u32);
            if self.chroma_format_idc == 3 {
                w.bit(false); // separate_colour_plane_flag
            }
            w.ue(0); // bit_depth_luma_minus8
            w.ue(0); // bit_depth_chroma_minus8
            w.bit(false); // qpprime_y_zero_transform_bypass_flag
            w.bit(false); // seq_scaling_matrix_present_flag
        }
        w.ue(self.log2_max_frame_num_minus4 as u32);
        w.ue(2); // pic_order_cnt_type
        w.ue(self.max_num_ref_frames as u32);
        w.bit(false); // gaps_in_frame_num_value_allowed_flag
        w.ue(self.mbs_w() - 1);
        w.ue(self.mbs_h() - 1); // pic_height_in_map_units_minus1 (frame_mbs_only)
        w.bit(true); // frame_mbs_only_flag
        w.bit(true); // direct_8x8_inference_flag
        let (right, bottom) = self.crop();
        let cropping = right != 0 || bottom != 0;
        w.bit(cropping);
        if cropping {
            w.ue(0);
            w.ue(right);
            w.ue(0);
            w.ue(bottom);
        }
        w.bit(true); // vui_parameters_present_flag
        // vui_parameters()
        w.bit(false); // aspect_ratio_info_present_flag (square pixels)
        w.bit(false); // overscan_info_present_flag
        w.bit(true); // video_signal_type_present_flag
        w.u(3, 5); // video_format: unspecified
        w.bit(self.full_range);
        w.bit(true); // colour_description_present_flag
        w.u(8, 1); // colour_primaries BT.709
        w.u(8, 1); // transfer_characteristics BT.709
        w.u(8, 1); // matrix_coefficients BT.709
        w.bit(false); // chroma_loc_info_present_flag
        w.bit(false); // timing_info_present_flag (damage-driven: no fixed rate)
        w.bit(false); // nal_hrd_parameters_present_flag
        w.bit(false); // vcl_hrd_parameters_present_flag
        w.bit(false); // pic_struct_present_flag
        w.bit(true); // bitstream_restriction_flag
        w.bit(true); // motion_vectors_over_pic_boundaries_flag
        w.ue(2); // max_bytes_per_pic_denom (E.2.1 default)
        w.ue(1); // max_bits_per_mb_denom (E.2.1 default)
        w.ue(16); // log2_max_mv_length_horizontal
        w.ue(16); // log2_max_mv_length_vertical
        w.ue(0); // max_num_reorder_frames
        w.ue(self.max_num_ref_frames as u32); // max_dec_frame_buffering
        w.trailing();
        w.into_bytes()
    }

    /// pic_parameter_set_rbsp() (id 0, SPS 0).
    pub fn pps_rbsp(&self) -> Vec<u8> {
        let mut w = BitWriter::default();
        w.ue(0); // pic_parameter_set_id
        w.ue(0); // seq_parameter_set_id
        w.bit(self.cabac); // entropy_coding_mode_flag
        w.bit(false); // bottom_field_pic_order_in_frame_present_flag
        w.ue(0); // num_slice_groups_minus1
        w.ue(0); // num_ref_idx_l0_default_active_minus1
        w.ue(0); // num_ref_idx_l1_default_active_minus1
        w.bit(false); // weighted_pred_flag
        w.u(2, 0); // weighted_bipred_idc
        w.se(self.pic_init_qp_minus26 as i32);
        w.se(0); // pic_init_qs_minus26
        w.se(self.chroma_qp_index_offset as i32);
        w.bit(true); // deblocking_filter_control_present_flag
        w.bit(false); // constrained_intra_pred_flag
        w.bit(false); // redundant_pic_cnt_present_flag
        if self.profile_idc >= 100 {
            w.bit(self.transform_8x8);
            w.bit(false); // pic_scaling_matrix_present_flag
            w.se(self.chroma_qp_index_offset as i32); // second_chroma_qp_index_offset
        }
        w.trailing();
        w.into_bytes()
    }

    /// SPS and PPS NAL units, Annex-B, ready to precede an IDR.
    pub fn parameter_sets(&self) -> Vec<u8> {
        let mut out = Vec::new();
        nal(3, NAL_SPS, &self.sps_rbsp(), &mut out);
        nal(3, NAL_PPS, &self.pps_rbsp(), &mut out);
        out
    }
}

/// The level a size needs at `fps` (Table A-1: MaxFS in macroblocks, MaxMBPS), from 3.1 up.
pub fn level_for(width: u32, height: u32, fps: u32) -> u8 {
    let fs = width.div_ceil(16) as u64 * height.div_ceil(16) as u64;
    let mbps = fs * fps.max(1) as u64;
    const LEVELS: [(u8, u64, u64); 9] = [
        (31, 3_600, 108_000),
        (32, 5_120, 216_000),
        (40, 8_192, 245_760),
        (42, 8_704, 522_240),
        (50, 22_080, 589_824),
        (51, 36_864, 983_040),
        (52, 36_864, 2_073_600),
        (60, 139_264, 4_177_920),
        (62, 139_264, 16_711_680),
    ];
    LEVELS.iter().find(|(_, max_fs, max_mbps)| fs <= *max_fs && mbps <= *max_mbps).map_or(62, |l| l.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(width: u32, height: u32) -> StreamParams {
        StreamParams {
            profile_idc: 100,
            chroma_format_idc: 1,
            level_idc: 51,
            width,
            height,
            log2_max_frame_num_minus4: 12,
            max_num_ref_frames: 1,
            cabac: true,
            transform_8x8: true,
            pic_init_qp_minus26: 0,
            chroma_qp_index_offset: 0,
            full_range: false,
        }
    }

    /// A reader for the tests: parses what the writer wrote, field by field.
    struct Bits<'a> {
        b: &'a [u8],
        at: usize,
    }
    impl Bits<'_> {
        fn u(&mut self, n: u32) -> u32 {
            let mut v = 0;
            for _ in 0..n {
                let bit = (self.b[self.at / 8] >> (7 - self.at % 8)) & 1;
                v = v << 1 | bit as u32;
                self.at += 1;
            }
            v
        }
        fn ue(&mut self) -> u32 {
            let mut zeros = 0;
            while self.u(1) == 0 {
                zeros += 1;
            }
            ((1u64 << zeros) - 1 + self.u(zeros) as u64) as u32
        }
        fn se(&mut self) -> i32 {
            let k = self.ue() as i64;
            if k % 2 == 1 { ((k + 1) / 2) as i32 } else { (-(k / 2)) as i32 }
        }
    }

    #[test]
    fn exp_golomb_vectors() {
        // 9.1 Table 9-2: 0 -> 1, 1 -> 010, 2 -> 011, 3 -> 00100, 7 -> 0001000
        let mut w = BitWriter::default();
        for v in [0, 1, 2, 3, 7] {
            w.ue(v);
        }
        w.trailing();
        assert_eq!(w.into_bytes(), vec![0b1010_0110, 0b0100_0001, 0b0001_0000]);
        let mut w = BitWriter::default();
        for v in [1, -1, 2, -2, 0] {
            w.se(v);
        }
        w.trailing();
        let bytes = w.into_bytes();
        let mut r = Bits { b: &bytes, at: 0 };
        assert_eq!([r.se(), r.se(), r.se(), r.se(), r.se()], [1, -1, 2, -2, 0]);
    }

    #[test]
    fn emulation_prevention() {
        let mut out = Vec::new();
        nal(3, NAL_SPS, &[0, 0, 1, 0, 0, 0, 5, 0, 0, 4], &mut out);
        assert_eq!(out, vec![0, 0, 0, 1, 0x67, 0, 0, 3, 1, 0, 0, 3, 0, 5, 0, 0, 4]);
    }

    #[test]
    fn sps_fields_parse_back() {
        let p = params(2560, 1440);
        let sps = p.sps_rbsp();
        let mut r = Bits { b: &sps, at: 0 };
        assert_eq!((r.u(8), r.u(8), r.u(8)), (100, 0, 51));
        assert_eq!(r.ue(), 0); // sps id
        assert_eq!((r.ue(), r.ue(), r.ue(), r.u(1), r.u(1)), (1, 0, 0, 0, 0));
        assert_eq!(r.ue(), 12); // log2_max_frame_num_minus4
        assert_eq!(r.ue(), 2); // poc type
        assert_eq!(r.ue(), 1); // max_num_ref_frames
        assert_eq!(r.u(1), 0); // gaps
        assert_eq!((r.ue() + 1, r.ue() + 1), (160, 90));
        assert_eq!((r.u(1), r.u(1), r.u(1)), (1, 1, 0)); // frame_mbs_only, direct8x8, no crop (1440 = 90 MBs)
        assert_eq!(r.u(1), 1); // vui
        assert_eq!((r.u(1), r.u(1), r.u(1), r.u(3), r.u(1), r.u(1)), (0, 0, 1, 5, 0, 1));
        assert_eq!((r.u(8), r.u(8), r.u(8)), (1, 1, 1));
        assert_eq!((r.u(1), r.u(1), r.u(1), r.u(1), r.u(1)), (0, 0, 0, 0, 0));
        assert_eq!(r.u(1), 1); // bitstream_restriction
        assert_eq!((r.u(1), r.ue(), r.ue(), r.ue(), r.ue(), r.ue(), r.ue()), (1, 2, 1, 16, 16, 0, 1));
        assert_eq!(r.u(1), 1); // stop bit
    }

    #[test]
    fn sps_crops_to_the_picture() {
        let p = params(1920, 1080);
        assert_eq!((p.mbs_w(), p.mbs_h(), p.crop()), (120, 68, (0, 4)));
        let sps = p.sps_rbsp();
        let mut r = Bits { b: &sps, at: 0 };
        r.u(24);
        for _ in 0..4 {
            r.ue();
        }
        r.u(2);
        for _ in 0..3 {
            r.ue();
        }
        r.u(1);
        assert_eq!((r.ue() + 1, r.ue() + 1), (120, 68));
        assert_eq!((r.u(1), r.u(1), r.u(1)), (1, 1, 1));
        assert_eq!((r.ue(), r.ue(), r.ue(), r.ue()), (0, 0, 0, 4));
    }

    #[test]
    fn pps_fields_parse_back() {
        let pps = params(1280, 720).pps_rbsp();
        let mut r = Bits { b: &pps, at: 0 };
        assert_eq!((r.ue(), r.ue(), r.u(1), r.u(1), r.ue(), r.ue(), r.ue()), (0, 0, 1, 0, 0, 0, 0));
        assert_eq!((r.u(1), r.u(2), r.se(), r.se(), r.se()), (0, 0, 0, 0, 0));
        assert_eq!((r.u(1), r.u(1), r.u(1)), (1, 0, 0));
        assert_eq!((r.u(1), r.u(1), r.se(), r.u(1)), (1, 0, 0, 1));
    }

    #[test]
    fn levels() {
        assert_eq!(level_for(1920, 1080, 60), 42);
        assert_eq!(level_for(2560, 1440, 60), 51);
        assert_eq!(level_for(3840, 2160, 60), 52);
        assert_eq!(level_for(7680, 2160, 60), 60);
    }
}
