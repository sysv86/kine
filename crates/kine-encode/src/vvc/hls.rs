//! H.266 / VVC high-level syntax writers: AUD, VPS, SPS, PPS, picture header
//! and slice header.
//!
//! The syntax follows ITU-T H.266 clause 7.3 as implemented by VTM 24.0, for a
//! deliberately small but fully conformant configuration:
//!
//! * single layer, single OLS, one temporal sub-layer;
//! * 4:2:0, 8-bit, BT.709 limited range;
//! * one tile, one rectangular slice per picture (the `pps_no_pic_partition`
//!   shortcut), no WPP / entry points;
//! * intra frames only (`ph_inter_slice_allowed_flag = 0`);
//! * quadtree-only partitioning (no MTT, no dual tree);
//! * transform skip enabled, every residual block coded with `transform_skip_flag = 1`;
//! * no SAO/ALF/LMCS/scaling lists/dependent quantisation/sign data hiding;
//! * deblocking explicitly disabled.
//!
//! Everything the encoder emits is a *subset* of this configuration; the
//! parameter sets cannot express anything else.

use super::bits::{rbsp_to_ebsp, BitReader, BitWriter};

/// VVC `nal_unit_type` values used by this encoder (H.266 Table 5).
pub const NAL_IDR_N_LP: u8 = 8;
pub const NAL_VPS: u8 = 14;
pub const NAL_SPS: u8 = 15;
pub const NAL_PPS: u8 = 16;
pub const NAL_PH: u8 = 19;
pub const NAL_AUD: u8 = 20;

/// Colour description carried in the VUI (BT.709, limited/studio range).
pub const BT709_PRIMARIES: u8 = 1;
pub const BT709_TRANSFER: u8 = 1;
pub const BT709_MATRIX: u8 = 1;

/// Static configuration of a VVC stream produced by this encoder.
#[derive(Debug, Clone)]
pub struct VvcStreamConfig {
    /// Coded picture width in luma samples.  Must be a multiple of the CTU size.
    pub width: u32,
    /// Coded picture height in luma samples.  Must be a multiple of the CTU size.
    pub height: u32,
    /// Right crop in luma samples (must be even); applied via the conformance window.
    pub crop_right: u32,
    /// Bottom crop in luma samples (must be even); applied via the conformance window.
    pub crop_bottom: u32,
    /// Slice QP (`0..=63`).
    pub qp: u8,
    /// `general_level_idc` (`16 * major + 3 * minor`, e.g. 51 = Level 3.1).
    pub level_idc: u8,
    /// `log2` of the CTU size (`5`, `6` or `7`).
    pub ctu_log2: u8,
    /// `log2` of the minimum luma coding block size (`2..=ctu_log2`).
    pub min_cb_log2: u8,
    /// `log2` of the maximum transform-skip block size (`2..=5`).
    pub ts_log2: u8,
}

impl VvcStreamConfig {
    pub fn ctu_size(&self) -> u32 {
        1 << self.ctu_log2
    }

    pub fn chroma_width(&self) -> u32 {
        self.width.div_ceil(2)
    }

    pub fn chroma_height(&self) -> u32 {
        self.height.div_ceil(2)
    }

    /// Visible (cropped) luma width.
    pub fn visible_width(&self) -> u32 {
        self.width.saturating_sub(self.crop_right)
    }

    /// Visible (cropped) luma height.
    pub fn visible_height(&self) -> u32 {
        self.height.saturating_sub(self.crop_bottom)
    }
}

/// Choose the smallest VVC level whose `MaxLumaPs` covers `width * height`.
///
/// Limits from H.266 Annex A, Table A.1 (also `ProfileTierLevel.cpp` in VTM).
pub fn select_level_idc(width: u32, height: u32) -> u8 {
    let samples = u64::from(width) * u64::from(height);
    // (level_idc, max luma samples per picture)
    const LEVELS: [(u8, u64); 13] = [
        (16, 36_864),      // 1
        (32, 122_880),     // 2
        (35, 245_760),     // 2.1
        (48, 552_960),     // 3
        (51, 983_040),     // 3.1
        (64, 2_228_224),   // 4
        (67, 2_228_224),   // 4.1
        (80, 8_912_896),   // 5
        (83, 8_912_896),   // 5.1
        (86, 8_912_896),   // 5.2
        (96, 35_651_584),  // 6
        (99, 35_651_584),  // 6.1
        (102, 35_651_584), // 6.2
    ];
    for (idc, max) in LEVELS {
        if samples <= max {
            return idc;
        }
    }
    102
}

/// `profile_tier_level(profileTierPresentFlag = 1, maxNumSubLayersMinus1 = 0)`
/// with `general_constraints_info()` present.
pub fn write_profile_tier_level(bw: &mut BitWriter, level_idc: u8, ctu_log2: u8) {
    // Main 10 profile, Main tier.
    bw.write_bits(1, 7); // general_profile_idc = 1 (Main 10)
    bw.write_bit(false); // general_tier_flag
    bw.write_bits(u32::from(level_idc), 8); // general_level_idc
    bw.write_bit(true); // ptl_frame_only_constraint_flag
    bw.write_bit(false); // ptl_multilayer_enabled_flag
    write_general_constraints_info(bw, ctu_log2);
    // No sub-layer level flags (maxNumSubLayersMinus1 == 0).
    bw.align_zero(); // ptl_reserved_zero_bit
    bw.write_bits(0, 8); // ptl_num_sub_profiles
}

/// `general_constraints_info()` for the configuration this encoder emits.
fn write_general_constraints_info(bw: &mut BitWriter, ctu_log2: u8) {
    bw.write_bit(true); // gci_present_flag

    // general
    bw.write_bit(true); // gci_intra_only_constraint_flag
    bw.write_bit(true); // gci_all_layers_independent_constraint_flag
    bw.write_bit(false); // gci_one_au_only_constraint_flag

    // picture format
    bw.write_bits(8, 4); // gci_sixteen_minus_max_bitdepth_constraint_idc (8-bit)
    bw.write_bits(2, 2); // gci_three_minus_max_chroma_format_constraint_idc (4:2:0)

    // NAL unit type related
    bw.write_bit(true); // gci_no_mixed_nalu_types_in_pic_constraint_flag
    bw.write_bit(true); // gci_no_trail_constraint_flag
    bw.write_bit(true); // gci_no_stsa_constraint_flag
    bw.write_bit(true); // gci_no_rasl_constraint_flag
    bw.write_bit(true); // gci_no_radl_constraint_flag
    bw.write_bit(false); // gci_no_idr_constraint_flag (we use IDR)
    bw.write_bit(true); // gci_no_cra_constraint_flag
    bw.write_bit(true); // gci_no_gdr_constraint_flag
    bw.write_bit(true); // gci_no_aps_constraint_flag
    bw.write_bit(true); // gci_no_idr_rpl_constraint_flag

    // tile, slice, subpicture partitioning
    bw.write_bit(true); // gci_one_tile_per_pic_constraint_flag
    bw.write_bit(false); // gci_pic_header_in_slice_header_constraint_flag (PH NAL used)
    bw.write_bit(true); // gci_one_slice_per_pic_constraint_flag
    bw.write_bit(false); // gci_no_rectangular_slice_constraint_flag
    bw.write_bit(false); // gci_one_slice_per_subpic_constraint_flag
    bw.write_bit(true); // gci_no_subpic_info_constraint_flag

    // CTU and block partitioning
    bw.write_bits(u32::from(3 - (ctu_log2 - 5)), 2); // gci_three_minus_max_log2_ctu_size_constraint_idc
    bw.write_bit(true); // gci_no_partition_constraints_override_constraint_flag
    bw.write_bit(true); // gci_no_mtt_constraint_flag
    bw.write_bit(true); // gci_no_qtbtt_dual_tree_intra_constraint_flag

    // intra
    bw.write_bit(true); // gci_no_palette_constraint_flag
    bw.write_bit(true); // gci_no_ibc_constraint_flag
    bw.write_bit(true); // gci_no_isp_constraint_flag
    bw.write_bit(true); // gci_no_mrl_constraint_flag
    bw.write_bit(true); // gci_no_mip_constraint_flag
    bw.write_bit(true); // gci_no_cclm_constraint_flag

    // inter
    bw.write_bit(true); // gci_no_ref_pic_resampling_constraint_flag
    bw.write_bit(true); // gci_no_res_change_in_clvs_constraint_flag
    bw.write_bit(true); // gci_no_weighted_prediction_constraint_flag
    bw.write_bit(true); // gci_no_ref_wraparound_constraint_flag
    bw.write_bit(true); // gci_no_temporal_mvp_constraint_flag
    bw.write_bit(true); // gci_no_sbtmvp_constraint_flag
    bw.write_bit(true); // gci_no_amvr_constraint_flag
    bw.write_bit(true); // gci_no_bdof_constraint_flag
    bw.write_bit(true); // gci_no_smvd_constraint_flag
    bw.write_bit(true); // gci_no_dmvr_constraint_flag
    bw.write_bit(true); // gci_no_mmvd_constraint_flag
    bw.write_bit(true); // gci_no_affine_motion_constraint_flag
    bw.write_bit(true); // gci_no_prof_constraint_flag
    bw.write_bit(true); // gci_no_bcw_constraint_flag
    bw.write_bit(true); // gci_no_ciip_constraint_flag
    bw.write_bit(true); // gci_no_gpm_constraint_flag

    // transform, quantisation, residual
    bw.write_bit(true); // gci_no_luma_transform_size_64_constraint_flag
    bw.write_bit(false); // gci_no_transform_skip_constraint_flag (transform skip allowed)
    bw.write_bit(true); // gci_no_bdpcm_constraint_flag
    bw.write_bit(true); // gci_no_mts_constraint_flag
    bw.write_bit(true); // gci_no_lfnst_constraint_flag
    bw.write_bit(true); // gci_no_joint_cbcr_constraint_flag
    bw.write_bit(true); // gci_no_sbt_constraint_flag
    bw.write_bit(true); // gci_no_act_constraint_flag
    bw.write_bit(true); // gci_no_explicit_scaling_list_constraint_flag
    bw.write_bit(true); // gci_no_dep_quant_constraint_flag
    bw.write_bit(true); // gci_no_sign_data_hiding_constraint_flag
    bw.write_bit(true); // gci_no_cu_qp_delta_constraint_flag
    bw.write_bit(true); // gci_no_chroma_qp_offset_constraint_flag

    // loop filter
    bw.write_bit(true); // gci_no_sao_constraint_flag
    bw.write_bit(true); // gci_no_alf_constraint_flag
    bw.write_bit(true); // gci_no_ccalf_constraint_flag
    bw.write_bit(true); // gci_no_lmcs_constraint_flag
    bw.write_bit(true); // gci_no_ladf_constraint_flag
    bw.write_bit(true); // gci_no_virtual_boundaries_constraint_flag

    bw.write_bits(0, 8); // gci_num_additional_bits

    bw.align_zero(); // gci_alignment_zero_bit
}

/// Build the VPS RBSP for a single-layer stream with `vps_video_parameter_set_id = 1`.
pub fn build_vps_rbsp(cfg: &VvcStreamConfig) -> Vec<u8> {
    let mut bw = BitWriter::with_capacity(24);
    bw.write_bits(1, 4); // vps_video_parameter_set_id = 1
    bw.write_bits(0, 6); // vps_max_layers_minus1
    bw.write_bits(0, 3); // vps_max_sublayers_minus1
    bw.write_bits(0, 6); // vps_layer_id[0]
                         // vps_ptl_reserved_zero_bit: zeros until byte aligned (19 -> 24 bits).
    bw.align_zero();
    write_profile_tier_level(&mut bw, cfg.level_idc, cfg.ctu_log2);
    bw.write_bit(false); // vps_extension_flag
    bw.rbsp_trailing_bits();
    bw.into_bytes()
}

/// Build the SPS RBSP.
pub fn build_sps_rbsp(cfg: &VvcStreamConfig) -> Vec<u8> {
    let mut bw = BitWriter::with_capacity(48);
    bw.write_bits(0, 4); // sps_seq_parameter_set_id
    bw.write_bits(1, 4); // sps_video_parameter_set_id
    bw.write_bits(0, 3); // sps_max_sub_layers_minus1
    bw.write_bits(1, 2); // sps_chroma_format_idc = 1 (4:2:0)
    bw.write_bits(u32::from(cfg.ctu_log2 - 5), 2); // sps_log2_ctu_size_minus5
    bw.write_bit(true); // sps_ptl_dpb_hrd_params_present_flag
    write_profile_tier_level(&mut bw, cfg.level_idc, cfg.ctu_log2);

    bw.write_bit(false); // sps_gdr_enabled_flag
    bw.write_bit(false); // sps_ref_pic_resampling_enabled_flag
    bw.write_ue(cfg.width); // sps_pic_width_max_in_luma_samples
    bw.write_ue(cfg.height); // sps_pic_height_max_in_luma_samples
    let crop = cfg.crop_right > 0 || cfg.crop_bottom > 0;
    bw.write_bit(crop); // sps_conformance_window_flag
    if crop {
        // Offsets are in chroma samples for 4:2:0.
        assert!(cfg.crop_right.is_multiple_of(2) && cfg.crop_bottom.is_multiple_of(2));
        bw.write_ue(0); // sps_conf_win_left_offset
        bw.write_ue(cfg.crop_right / 2); // sps_conf_win_right_offset
        bw.write_ue(0); // sps_conf_win_top_offset
        bw.write_ue(cfg.crop_bottom / 2); // sps_conf_win_bottom_offset
    }
    bw.write_bit(false); // sps_subpic_info_present_flag
    bw.write_ue(0); // sps_bitdepth_minus8 (8-bit)
    bw.write_bit(false); // sps_entropy_coding_sync_enabled_flag
    bw.write_bit(false); // sps_entry_point_offsets_present_flag
    bw.write_bits(4, 4); // sps_log2_max_pic_order_cnt_lsb_minus4 => 8-bit POC LSB
    bw.write_bit(false); // sps_poc_msb_cycle_flag
    bw.write_bits(0, 2); // sps_num_extra_ph_bytes
    bw.write_bits(0, 2); // sps_num_extra_sh_bytes

    // dpb_parameters(maxSubLayersMinus1 = 0, ...)
    bw.write_ue(0); // dpb_max_dec_pic_buffering_minus1[0]
    bw.write_ue(0); // dpb_max_num_reorder_pics[0]
    bw.write_ue(0); // dpb_max_latency_increase_plus1[0]

    bw.write_ue(u32::from(cfg.min_cb_log2 - 2)); // sps_log2_min_luma_coding_block_size_minus2
    bw.write_bit(false); // sps_partition_constraints_override_enabled_flag
    bw.write_ue(0); // sps_log2_diff_min_qt_min_cb_intra_slice_luma
    bw.write_ue(0); // sps_max_mtt_hierarchy_depth_intra_slice_luma
    bw.write_bit(false); // sps_qtbtt_dual_tree_intra_flag
    bw.write_ue(0); // sps_log2_diff_min_qt_min_cb_inter_slice
    bw.write_ue(0); // sps_max_mtt_hierarchy_depth_inter_slice
    if cfg.ctu_log2 > 5 {
        bw.write_bit(false); // sps_max_luma_transform_size_64_flag (max TB 32x32)
    }

    bw.write_bit(true); // sps_transform_skip_enabled_flag
    bw.write_ue(u32::from(cfg.ts_log2 - 2)); // sps_log2_transform_skip_max_size_minus2
    bw.write_bit(false); // sps_bdpcm_enabled_flag
    bw.write_bit(false); // sps_mts_enabled_flag
    bw.write_bit(false); // sps_lfnst_enabled_flag

    // Chroma tools: joint CbCr off, one identity-ish chroma QP table
    // (start=0, one point, delta 0/0).  Note: the derived table is *not* the
    // identity (qp >= 27 maps to qp - 1); [`derived_chroma_qp`] mirrors it.
    bw.write_bit(false); // sps_joint_cbcr_enabled_flag
    bw.write_bit(true); // sps_same_qp_table_for_chroma_flag
    bw.write_se(0); // sps_qp_table_starts_minus26
    bw.write_ue(0); // sps_num_points_in_qp_table_minus1
    bw.write_ue(0); // sps_delta_qp_in_val_minus1[0][0]
    bw.write_ue(0); // sps_delta_qp_diff_val[0][0]

    bw.write_bit(false); // sps_sao_enabled_flag
    bw.write_bit(false); // sps_alf_enabled_flag
    bw.write_bit(false); // sps_lmcs_enable_flag
    bw.write_bit(false); // sps_weighted_pred_flag
    bw.write_bit(false); // sps_weighted_bipred_flag
    bw.write_bit(false); // sps_long_term_ref_pics_flag
    bw.write_bit(false); // sps_inter_layer_prediction_enabled_flag (vps_id > 0)
    bw.write_bit(false); // sps_idr_rpl_present_flag
    bw.write_bit(true); // sps_rpl1_same_as_rpl0_flag
    bw.write_ue(0); // sps_num_ref_pic_lists[0]
    bw.write_bit(false); // sps_ref_wraparound_enabled_flag
    bw.write_bit(false); // sps_temporal_mvp_enabled_flag
    bw.write_bit(false); // sps_amvr_enabled_flag
    bw.write_bit(false); // sps_bdof_enabled_flag
    bw.write_bit(false); // sps_smvd_enabled_flag
    bw.write_bit(false); // sps_dmvr_enabled_flag
    bw.write_bit(false); // sps_mmvd_enabled_flag
    bw.write_ue(5); // sps_six_minus_max_num_merge_cand => MaxNumMergeCand == 1
    bw.write_bit(false); // sps_sbt_enabled_flag
    bw.write_bit(false); // sps_affine_enabled_flag
    bw.write_bit(false); // sps_bcw_enabled_flag
    bw.write_bit(false); // sps_ciip_enabled_flag
                         // sps_gpm_enabled_flag is *not* read because MaxNumMergeCand < 2.
    bw.write_ue(0); // sps_log2_parallel_merge_level_minus2
    bw.write_bit(false); // sps_isp_enabled_flag
    bw.write_bit(false); // sps_mrl_enabled_flag
    bw.write_bit(false); // sps_mip_enabled_flag
    bw.write_bit(false); // sps_cclm_enabled_flag
    bw.write_bit(true); // sps_chroma_horizontal_collocated_flag
    bw.write_bit(true); // sps_chroma_vertical_collocated_flag
    bw.write_bit(false); // sps_palette_enabled_flag
    bw.write_ue(0); // sps_internal_bit_depth_minus_input_bit_depth
    bw.write_bit(false); // sps_ibc_enabled_flag
    bw.write_bit(false); // sps_ladf_enabled_flag
    bw.write_bit(false); // sps_explicit_scaling_list_enabled_flag
    bw.write_bit(false); // sps_dep_quant_enabled_flag
    bw.write_bit(false); // sps_sign_data_hiding_enabled_flag
    bw.write_bit(false); // sps_virtual_boundaries_enabled_flag
    bw.write_bit(false); // sps_timing_hrd_params_present_flag
    bw.write_bit(false); // sps_field_seq_flag
    bw.write_bit(true); // sps_vui_parameters_present_flag
    bw.write_ue(4); // sps_vui_payload_size_minus1 => 5-byte payload
    bw.align_zero(); // sps_vui_alignment_zero_bit
    let vui = build_vui_payload();
    assert_eq!(vui.len(), 5);
    bw.append_bytes(&vui);
    bw.write_bit(false); // sps_extension_present_flag
    bw.rbsp_trailing_bits();
    bw.into_bytes()
}

/// 5-byte `vui_parameters()` payload: progressive BT.709 limited-range 4:2:0.
fn build_vui_payload() -> Vec<u8> {
    let mut bw = BitWriter::with_capacity(8);
    bw.write_bit(true); // vui_progressive_source_flag
    bw.write_bit(false); // vui_interlaced_source_flag
    bw.write_bit(true); // vui_non_packed_constraint_flag
    bw.write_bit(true); // vui_non_projected_constraint_flag
    bw.write_bit(false); // vui_aspect_ratio_info_present_flag
    bw.write_bit(false); // vui_overscan_info_present_flag
    bw.write_bit(true); // vui_colour_description_present_flag
    bw.write_bits(u32::from(BT709_PRIMARIES), 8); // vui_colour_primaries
    bw.write_bits(u32::from(BT709_TRANSFER), 8); // vui_transfer_characteristics
    bw.write_bits(u32::from(BT709_MATRIX), 8); // vui_matrix_coeffs
    bw.write_bit(false); // vui_full_range_flag (studio swing)
    bw.write_bit(true); // vui_chroma_loc_info_present_flag
    bw.write_ue(0); // vui_chroma_sample_loc_type (left / co-sited horizontally)
                    // The decoder expects `vui_payload_bit_equal_to_one` then zeros up to the
                    // end of the length-prefixed payload.
    bw.write_bit(true);
    while !bw.is_byte_aligned() {
        bw.write_bit(false);
    }
    bw.into_bytes()
}

/// Build the PPS RBSP (`pps_no_pic_partition_flag = 1`, deblocking disabled).
pub fn build_pps_rbsp(cfg: &VvcStreamConfig) -> Vec<u8> {
    let mut bw = BitWriter::with_capacity(24);
    bw.write_bits(0, 6); // pps_pic_parameter_set_id
    bw.write_bits(0, 4); // pps_seq_parameter_set_id
    bw.write_bit(false); // pps_mixed_nalu_types_in_pic_flag
    bw.write_ue(cfg.width); // pps_pic_width_in_luma_samples
    bw.write_ue(cfg.height); // pps_pic_height_in_luma_samples
    bw.write_bit(false); // pps_conformance_window_flag
    bw.write_bit(false); // pps_scaling_window_explicit_signalling_flag
    bw.write_bit(false); // pps_output_flag_present_flag
    bw.write_bit(true); // pps_no_pic_partition_flag
    bw.write_bit(false); // pps_subpic_id_mapping_present_flag
    bw.write_bit(false); // pps_cabac_init_present_flag
    bw.write_ue(0); // pps_num_ref_idx_default_active_minus1[0]
    bw.write_ue(0); // pps_num_ref_idx_default_active_minus1[1]
    bw.write_bit(false); // pps_rpl1_idx_present_flag
    bw.write_bit(false); // pps_weighted_pred_flag
    bw.write_bit(false); // pps_weighted_bipred_flag
    bw.write_bit(false); // pps_ref_wraparound_enabled_flag
    bw.write_se(i32::from(cfg.qp) - 26); // pps_init_qp_minus26
    bw.write_bit(false); // pps_cu_qp_delta_enabled_flag
    bw.write_bit(false); // pps_chroma_tool_offsets_present_flag
    bw.write_bit(true); // pps_deblocking_filter_control_present_flag
    bw.write_bit(false); // pps_deblocking_filter_override_enabled_flag
    bw.write_bit(true); // pps_deblocking_filter_disabled_flag
    bw.write_bit(false); // pps_picture_header_extension_present_flag
    bw.write_bit(false); // pps_slice_header_extension_present_flag
    bw.write_bit(false); // pps_extension_flag
    bw.rbsp_trailing_bits();
    bw.into_bytes()
}

/// Build the picture header RBSP.
///
/// `ph_inter_slice_allowed_flag = 0` makes `ph_intra_slice_allowed_flag` and
/// `sh_slice_type` inferred, which keeps the header minimal while remaining
/// conformant (the only slice type a decoder can then use is I).
pub fn build_ph_rbsp(poc_lsb: u32) -> Vec<u8> {
    let mut bw = BitWriter::with_capacity(4);
    bw.write_bit(true); // ph_gdr_or_irap_pic_flag
    bw.write_bit(false); // ph_non_ref_pic_flag
    bw.write_bit(false); // ph_gdr_pic_flag (IRAP, not GDR)
    bw.write_bit(false); // ph_inter_slice_allowed_flag
    bw.write_ue(0); // ph_pic_parameter_set_id
    bw.write_bits(poc_lsb & 0xFF, 8); // ph_pic_order_cnt_lsb
    bw.rbsp_trailing_bits();
    bw.into_bytes()
}

/// Build the slice header bits (without `slice_data()`), including the
/// trailing `byte_alignment()`.
pub fn build_slice_header_bits(slice_qp_delta: i32) -> Vec<u8> {
    let mut bw = BitWriter::with_capacity(8);
    bw.write_bit(false); // sh_picture_header_in_slice_header_flag (PH NAL used)
    bw.write_bit(false); // sh_no_output_of_prior_pics_flag (IDR_N_LP)
    bw.write_se(slice_qp_delta); // sh_qp_delta
    bw.write_bit(false); // sh_ts_residual_coding_disabled_flag = 0 (TS residual coding)
    bw.byte_alignment();
    bw.into_bytes()
}

/// Build an AUD RBSP (`aud_irap_or_gdr_au_flag = 1`, I picture).
pub fn build_aud_rbsp() -> Vec<u8> {
    let mut bw = BitWriter::with_capacity(2);
    bw.write_bit(true); // aud_irap_or_gdr_au_flag
    bw.write_bits(0, 3); // aud_pic_type = 0 (I)
    bw.rbsp_trailing_bits();
    bw.into_bytes()
}

/// Append one Annex-B NAL unit: 4-byte start code, 2-byte VVC header, EPB-escaped RBSP.
pub fn push_nal(out: &mut Vec<u8>, nal_type: u8, layer_id: u8, temporal_id_plus1: u8, rbsp: &[u8]) {
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    out.push(layer_id & 0x3F); // forbidden_zero_bit=0 | nuh_reserved_zero_bit=0 | nuh_layer_id
    out.push(((nal_type & 0x1F) << 3) | (temporal_id_plus1 & 0x07));
    out.extend_from_slice(&rbsp_to_ebsp(rbsp));
}

/// The chroma QP mapping table derived from the trivial parameter set this
/// encoder writes (`start = 0`, one point, `delta_qp_in_val_minus1 = 0`,
/// `delta_qp_diff_val = 0`).
///
/// Mirrors `ChromaQpMappingTable::deriveChromaQPMappingTables()`: for 8-bit the
/// derived table is `q -> q` for `q <= 26` and `q -> q - 1` above that.
pub fn derived_chroma_qp(qp: i32) -> i32 {
    let qpi = qp.clamp(0, 63);
    if qpi <= 26 {
        qpi
    } else {
        qpi - 1
    }
}

/// Information the muxer needs from an SPS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VvcSpsInfo {
    pub sps_id: u8,
    pub vps_id: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    /// Visible (conformance-window-cropped) size.
    pub width: u32,
    pub height: u32,
    /// Coded size (`sps_pic_width_max_in_luma_samples`).
    pub coded_width: u32,
    pub coded_height: u32,
    pub ctu_log2: u8,
    pub profile_idc: u8,
    pub tier_flag: bool,
    pub level_idc: u8,
    pub frame_only_constraint: bool,
    pub multilayer_enabled: bool,
    /// `general_constraints_info()` syntax bits, starting with
    /// `gci_present_flag` and excluding its alignment zeros.
    pub gci_bits: Vec<bool>,
    pub num_sub_profiles: u8,
    pub sub_profile_idc: Vec<u32>,
}

/// Parse the leading fields of an SPS RBSP (no emulation-prevention bytes).
///
/// Covers the syntax this encoder emits plus the common optional picture-size
/// fields.  Returns `None` for structures it cannot follow (sub-pictures /
/// extra PH or SH bytes), because the muxer only needs best-effort metadata.
pub fn parse_sps_info(rbsp: &[u8]) -> Option<VvcSpsInfo> {
    let mut br = BitReader::new(rbsp);
    let sps_id = br.read_bits(4).ok()? as u8;
    let vps_id = br.read_bits(4).ok()? as u8;
    let _max_sub_layers_minus1 = br.read_bits(3).ok()?;
    let chroma_format_idc = br.read_bits(2).ok()? as u8;
    let log2_ctu_size_minus5 = br.read_bits(2).ok()? as u8;
    let ptl_present = br.read_bit().ok()?;
    if !ptl_present {
        return None;
    }

    // profile_tier_level(1, max_sub_layers_minus1)
    let profile_idc = br.read_bits(7).ok()? as u8;
    let tier_flag = br.read_bit().ok()?;
    let level_idc = br.read_bits(8).ok()? as u8;
    let frame_only_constraint = br.read_bit().ok()?;
    let multilayer_enabled = br.read_bit().ok()?;

    // general_constraints_info(): capture the syntax bits so the muxer can
    // re-emit them inside a VvcPTLRecord, where the enclosing bit offset (and
    // therefore the alignment padding) differs from the SPS.
    let mut gci_bits = Vec::with_capacity(96);
    let gci_present = br.read_bit().ok()?;
    gci_bits.push(gci_present);
    if gci_present {
        // 71 constraint flags
        const GCI_FLAGS: usize = 71;
        for _ in 0..GCI_FLAGS {
            gci_bits.push(br.read_bit().ok()?);
        }
        let additional = br.read_bits(8).ok()?;
        for i in (0..8).rev() {
            gci_bits.push((additional >> i) & 1 != 0);
        }
        if additional > 5 {
            for _ in 0..6 {
                gci_bits.push(br.read_bit().ok()?);
            }
        }
        for _ in 0..additional.saturating_sub(if additional > 5 { 6 } else { 0 }) {
            gci_bits.push(br.read_bit().ok()?);
        }
    }
    while !br.is_byte_aligned() {
        if br.read_bit().ok()? {
            return None; // gci_alignment_zero_bit must be zero
        }
    }

    // sub_layer_level_present_flag loop is empty for max_sub_layers_minus1 == 0,
    // but be tolerant: skip nothing if sub-layers exist (rare, unsupported).
    let num_sub_profiles = br.read_bits(8).ok()? as u8;
    let mut sub_profile_idc = Vec::with_capacity(num_sub_profiles as usize);
    for _ in 0..num_sub_profiles {
        sub_profile_idc.push(br.read_bits(32).ok()?);
    }

    let _gdr_enabled = br.read_bit().ok()?;
    let _ref_pic_resampling = br.read_bit().ok()?;
    let width = br.read_ue().ok()?;
    let height = br.read_ue().ok()?;
    let conformance_window = br.read_bit().ok()?;
    let (mut crop_right, mut crop_bottom) = (0u32, 0u32);
    if conformance_window {
        let _left = br.read_ue().ok()?;
        crop_right = br.read_ue().ok()?;
        let _top = br.read_ue().ok()?;
        crop_bottom = br.read_ue().ok()?;
    }
    let _subpic_info = br.read_bit().ok()?;
    let bit_depth_luma = br.read_ue().ok()? as u8 + 8;

    // Conformance-window offsets are in chroma units for 4:2:0.
    let sub_w = if chroma_format_idc == 1 { 2 } else { 1 };
    Some(VvcSpsInfo {
        sps_id,
        vps_id,
        chroma_format_idc,
        bit_depth_luma,
        width: width.saturating_sub(crop_right * sub_w),
        height: height.saturating_sub(crop_bottom * sub_w),
        coded_width: width,
        coded_height: height,
        ctu_log2: log2_ctu_size_minus5 + 5,
        profile_idc,
        tier_flag,
        level_idc,
        frame_only_constraint,
        multilayer_enabled,
        gci_bits,
        num_sub_profiles,
        sub_profile_idc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_config() -> VvcStreamConfig {
        VvcStreamConfig {
            width: 128,
            height: 128,
            crop_right: 0,
            crop_bottom: 0,
            qp: 26,
            level_idc: 16,
            ctu_log2: 6,
            min_cb_log2: 3,
            ts_log2: 3,
        }
    }

    #[test]
    fn ptl_matches_the_vtm_validated_reference() {
        // PTL bytes from HLS.md section 14 (128x128, CTU 64, GCI present,
        // Level 1).  This pins the entire 71-flag constraint block.  The only
        // deliberate difference is byte 3: the reference stream claimed a
        // maximum chroma format of 4:2:2 (idc 2, `01b`), while this encoder
        // constrains it tightly to 4:2:0 (idc 1, `10b`) — both are legal, ours
        // is the honest constraint for a 4:2:0 stream.
        let expected = [
            0x02u8, 0x10, 0xBA, 0x2F, 0xBE, 0x9B, 0xFF, 0xFF, 0xFF, 0x7F, 0xFF, 0xC0, 0x00, 0x00,
        ];
        let mut bw = BitWriter::new();
        write_profile_tier_level(&mut bw, 16, 6);
        assert_eq!(bw.into_bytes(), expected);
    }

    #[test]
    fn aud_ph_and_slice_header_bytes() {
        assert_eq!(build_aud_rbsp(), vec![0x88]);
        // gdr_or_irap=1, non_ref=0, gdr=0, inter_allowed=0, ue(0), poc=0, trailing
        assert_eq!(build_ph_rbsp(0), vec![0x88, 0x04]);
        // ph_in_sh=0, no_output=0, se(0)=1, ts_disabled=0, alignment => 0x28
        assert_eq!(build_slice_header_bits(0), vec![0x28]);
    }

    #[test]
    fn vps_and_sps_have_expected_prefix() {
        let cfg = reference_config();
        let vps = build_vps_rbsp(&cfg);
        assert_eq!(&vps[..3], &[0x10, 0x00, 0x00]);
        let sps = build_sps_rbsp(&cfg);
        assert_eq!(&sps[..3], &[0x01, 0x0B, 0x02]);
        let info = parse_sps_info(&sps).expect("parse own SPS");
        assert_eq!((info.width, info.height), (128, 128));
        assert_eq!(info.level_idc, 16);
        assert_eq!(info.profile_idc, 1);
    }

    #[test]
    fn conformance_window_crops_visible_size() {
        let mut cfg = reference_config();
        cfg.width = 128;
        cfg.height = 64;
        cfg.crop_right = 28;
        cfg.crop_bottom = 4;
        let sps = build_sps_rbsp(&cfg);
        let info = parse_sps_info(&sps).expect("parse cropped SPS");
        assert_eq!((info.width, info.height), (100, 60));
    }

    #[test]
    fn chroma_qp_table_matches_reference_derivation() {
        assert_eq!(derived_chroma_qp(0), 0);
        assert_eq!(derived_chroma_qp(26), 26);
        assert_eq!(derived_chroma_qp(27), 26);
        assert_eq!(derived_chroma_qp(63), 62);
    }
}
