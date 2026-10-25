# Kine's H.266 / VVC bitstream

`crates/kine-encode/src/vvc` contains a real ITU-T H.266 | ISO/IEC 23090-3
encoder — not a private pixel container with VVC-looking NAL headers. The
output is Annex-B and is decoded byte-exactly by VVdeC, FFmpeg's native VVC
decoder and the VTM reference decoder.

It intentionally implements a **small, conformant subset** of VVC. Everything
below is normative for the streams Kine writes.

## What is coded

| tool | decision |
|---|---|
| profile / tier / level | Main 10 (`general_profile_idc = 1`), Main tier, smallest Annex A level that covers the picture |
| chroma / bit depth | 4:2:0, 8-bit, BT.709 limited range (also in the VUI) |
| picture type | one `IDR_N_LP` per access unit; no inter prediction at all |
| CTU | 32×32, `sps_log2_ctu_size_minus5 = 0` |
| min CU | 32×32, `sps_log2_min_luma_coding_block_size_minus2 = 3` |
| partitioning | quadtree-only, `sps_max_mtt_hierarchy_depth_* = 0`; since `min CU == CTU`, **no `split_cu_flag` is ever emitted** |
| tree | single tree, `sps_qtbtt_dual_tree_intra_flag = 0` |
| slice / tile | one rectangular slice, one tile (`pps_no_pic_partition_flag = 1`), no WPP, no entry points |
| intra mode | PLANAR for every CU, signalled as `intra_luma_mpm_flag = 1` → `intra_luma_not_planar_flag = 0`; chroma uses `intra_chroma_pred_mode = 0` (DM ⇒ PLANAR) |
| transform | transform skip for every component; `sps_log2_transform_skip_max_size_minus2 = 3` (32×32) |
| residual syntax | `residual_ts_coding()` (`sh_ts_residual_coding_disabled_flag = 0`) |
| quantisation | `SliceQpY = pps_init_qp_minus26 + 26 + sh_qp_delta`; QP 4 is the lossless point (`dequant(level) == level`) |
| in-loop filters | deblocking explicitly disabled (`present=1, override=0, disabled=1`), SAO/ALF/LMCS off |
| other tools | MTS, LFNST, MIP, ISP, BDPCM, palette, IBC, ACT, CCLM, DQ, sign data hiding, cu_qp_delta, chroma-QP offsets, sub-pictures, scaling lists all off |

Because `CU == CTU`, one coding unit always produces exactly one transform
unit: luma 32×32, chroma 16×16. That removes the entire transform-tree and
coding-tree recursion from the encoder without violating any constraint.

## Access-unit layout

```
AUD (20)  VPS (14)  SPS (15)  PPS (16)  PH (19)  IDR_N_LP VCL (8)
```

All NAL units have `nuh_layer_id = 0`, `nuh_temporal_id_plus1 = 1`. Parameter
sets are repeated in-band on every IDR so a raw Annex-B file is
self-contained; the ISO-BMFF `vvcC` and Matroska `CodecPrivate` carry the same
sets out-of-band.

Each access unit is an IDR, so `ph_pic_order_cnt_lsb = 0` and
`sh_no_output_of_prior_pics_flag = 0`.

### Parameter-set specifics

* **VPS** (`vps_video_parameter_set_id = 1`, `sps_video_parameter_set_id = 1`):
  one layer, one temporal sub-layer, one PTL.
* **SPS**: no sub-pictures; `sps_log2_max_pic_order_cnt_lsb_minus4 = 4`
  (8-bit POC LSB); DPB `1 / 0 / 0`; no RPL entries; `sps_six_minus_max_num_merge_cand = 5`
  (which also suppresses `sps_gpm_enabled_flag`); identity-ish chroma QP table
  signalled with `start=0, one point, delta 0/0`; 5-byte VUI payload with BT.709
  colour description and chroma location.
* **Conformance window**: the picture is padded to a multiple of 32 luma
  samples and the SPS conformance window crops it back. Offsets are in chroma
  samples, so the visible size is rounded down to an even number of luma
  samples (documented limitation for odd render sizes).
* **PPS**: `pps_no_pic_partition_flag = 1`, no chroma tool offsets, no
  `cu_qp_delta`, no scaling lists, deblocking disabled as above.
* **PH**: `ph_inter_slice_allowed_flag = 0`, so `sh_slice_type` is inferred
  I and the inter-slice PH block is absent.
* **SH**: `sh_picture_header_in_slice_header_flag = 0`, `sh_qp_delta = 0`,
  `sh_ts_residual_coding_disabled_flag = 0`, then `byte_alignment()`.

The PTL carries `general_constraints_info()` with the constraints that match
this stream (intra-only, one tile/slice, no MTT/sub-pictures, no SAO/ALF/LMCS,
transform skip allowed, 32×32 CTU, 8-bit 4:2:0). `general_level_idc` is chosen
from Annex A `MaxLumaPs`.

## Slice data

Per CTU the bin order is exactly:

```
intra_luma_mpm_flag            = 1   Ctx::IntraLumaMpmFlag
intra_luma_not_planar_flag     = 0   Ctx::IntraLumaPlanarFlag(1)   -> PLANAR
intra_chroma_pred_mode         = 0   Ctx::IntraChromaPredMode(0)   -> DM
cbf_cb                               Ctx::QtCbf[Cb](0)
cbf_cr                               Ctx::QtCbf[Cr](cbf_cb)
cbf_luma                             Ctx::QtCbf[Y](0)
  if cbf_luma: transform_skip_flag=1 (Ctx::TransformSkipFlag(0)), residual_ts_coding(Y)
  if cbf_cb:   transform_skip_flag=1 (Ctx::TransformSkipFlag(1)), residual_ts_coding(Cb)
  if cbf_cr:   transform_skip_flag=1 (Ctx::TransformSkipFlag(1)), residual_ts_coding(Cr)
```

There is no `cu_skip_flag`, `pred_mode_flag`, `rqt_root_cbf`,
`split_transform_flag`, `cu_qp_delta`, `mts_idx` or per-CU terminator in this
configuration. The slice ends with `encodeBinTrm(1)`, CABAC flush and
`byte_alignment()`.

### Transform-skip residual coding

`residual_ts_coding()` is implemented bit-exactly from VTM 24.0:

* 4×4 coefficient groups in forward grouped-diagonal order;
* per group, `sb_coded_flag` (`Ctx::TsSigCoeffGroup(sigLeft+sigAbove)`), with
  the last group inferred when it is the only coded group;
* pass 1: significance (last scan position inferred), context-coded sign
  (`Ctx::TsResidualSign`), `gt1` (`Ctx::TsLrg1Flag`) and parity
  (`Ctx::TsParFlag`);
* pass 2: four `gt2..gt5` bins (`Ctx::TsGtxFlag(1..4)`);
* pass 3: Go-Rice remainder with `k = 1`, `COEF_REMAIN_BIN_REDUCTION = 5`,
  `maxLog2TrDynamicRange = 15`, plus the escape path once the per-TB context
  bin budget `(width*height*7)>>2` is exhausted;
* the transform-skip level remapping (`deriveModCoeff` / `decDeriveModCoeff`)
  uses the left and above neighbours.

Context models are initialised with the VVC two-state probability model and
the per-context log2 window size for an I slice (`initId = 2`).

## Reconstruction

PLANAR prediction follows VTM's integer formula, including the `[1 2 1]`
reference filter for luma planar blocks (`W*H > 32`) and the mandatory PDPC
step for planar/DC. Reference availability is resolved for a CTU-shaped
prediction block: above/above-right come from the fully decoded previous CTU
row, left comes from the fully decoded left CTU, and **below-left is padded**
(it lies in the next CTU row, which is not decoded yet in raster order).

Reconstruction is `Clip3(pred + dequant(level), 0, 255)`. Transform-skip
dequantisation uses `rightShift = 6 - QP/6`, the inverse quant scale table
`{40,45,51,57,64,72}`, round-half-up and the normative 15-bit clip.

## Muxing

* MP4: `vvc1` sample entry + `vvcC` (`VvcDecoderConfigurationRecord`). The
  record's PTL fields and `general_constraints_info` bits are read from the SPS,
  and arrays are `VPS`, `SPS`, `PPS` (OPI/DCI only when present, without a
  `num_nalus` field).
* Matroska: `V_MPEGI/ISO/VVC` (the registered Matroska Codec ID — note the
  `I`, not `H`) with the same `vvcC` record as `CodecPrivate`.

## Limitations

* Intra-only: no P/B slices, no motion estimation. Every frame is an IDR, so
  files are larger than a normal VVC encode and `--quality` controls QP rather
  than inter/intra trade-offs.
* One PLANAR mode only; no angular/DC selection, no MPM list derivation is
  needed because PLANAR is always selected through `mpm_flag = 1`.
* Dimensions are padded to 32×32 CTUs and cropped by the conformance window;
  odd visible dimensions are rounded down to even (a 4:2:0 conformance window
  cannot express a half chroma sample).
* The bundled `decode_vvc_annexb_to_i420` is a decoder for exactly this
  subset, not a general VVC decoder. Use VVdeC/FFmpeg/VTM for arbitrary
  bitstreams.

## Verification

`scripts/verify-vvc.sh` builds the `vvc_dump` example and compares decoded
output with the source (QP 4) or with Kine's own reconstruction at lossy QP:

* VVdeC 3.3.0-dev: byte-exact on every tested size from 2×2 to 1920×1080,
  lossless and lossy;
* FFmpeg n9.0.2 native VVC decoder: byte-exact, both Annex-B and MP4/MKV;
* VTM 24.0 `DecoderApp`: byte-exact, no conformance errors.

VTM and VVdeC report the expected picture size, `I-SLICE, QP 4` and the VUI
BT.709 colour description.
