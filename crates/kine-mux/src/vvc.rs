//! ISO/IEC 14496-15 Section 11 H.266 / VVC (`vvc1` / `vvcC`) packetizer and
//! ISO-BMFF `.mp4` muxer backend.

use crate::{frame_duration, AudioSpec, MuxError, VIDEO_TIMESCALE};
use std::io::Write;

/// Parameter set NAL units extracted from an H.266 / VVC Annex-B bitstream.
#[derive(Debug, Default, Clone)]
pub struct VvcParamSets {
    pub opi: Option<Vec<u8>>,
    pub dci: Option<Vec<u8>>,
    pub vps: Option<Vec<u8>>,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
}

impl VvcParamSets {
    pub fn has_core_sets(&self) -> bool {
        self.sps.is_some() && self.pps.is_some()
    }
}

/// Convert an H.266 / VVC Annex-B access unit into 4-byte length-prefixed NAL units
/// and extract any in-band `OPI` (12), `DCI` (13), `VPS` (14), `SPS` (15), and `PPS` (16) NALs.
pub fn annexb_to_vvcc_packets(data: &[u8]) -> (VvcParamSets, Vec<u8>) {
    let mut params = VvcParamSets::default();
    let mut out = Vec::with_capacity(data.len());
    let mut nals = Vec::new();

    let mut i = 0usize;
    let mut nal_start: Option<usize> = None;
    while i < data.len() {
        let sc_len = if i + 4 <= data.len() && &data[i..i + 4] == &[0, 0, 0, 1] {
            4
        } else if i + 3 <= data.len() && &data[i..i + 3] == &[0, 0, 1] {
            3
        } else {
            0
        };
        if sc_len > 0 {
            if let Some(start) = nal_start {
                if start < i {
                    nals.push(&data[start..i]);
                }
            }
            i += sc_len;
            nal_start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(start) = nal_start {
        if start < data.len() {
            nals.push(&data[start..]);
        }
    }

    for nal in nals {
        if nal.len() < 2 {
            continue;
        }
        let nal_type = (nal[1] >> 3) & 0x1F;
        match nal_type {
            12 if params.opi.is_none() => params.opi = Some(nal.to_vec()),
            13 if params.dci.is_none() => params.dci = Some(nal.to_vec()),
            14 if params.vps.is_none() => params.vps = Some(nal.to_vec()),
            15 if params.sps.is_none() => params.sps = Some(nal.to_vec()),
            16 if params.pps.is_none() => params.pps = Some(nal.to_vec()),
            _ => {}
        }
        let len = nal.len() as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);
    }

    (params, out)
}

/// Build an ISO/IEC 14496-15:2022 Section 11.2.4.2.2 `VvcDecoderConfigurationRecord` (`vvcC` payload).
pub fn build_vvcc_record(params: &VvcParamSets, width: u32, height: u32) -> Vec<u8> {
    let level_idc = params
        .sps
        .as_ref()
        .and_then(|sps| sps.get(5).copied())
        .unwrap_or(51);

    let mut rec = Vec::with_capacity(128);
    // Byte 0: reserved('11111'b) | LengthSizeMinusOne(3 => 4-byte NAL length) | ptl_present_flag(1)
    rec.push(0b1111_1111);
    // Bytes 1..=2: ols_idx(9 bits = 0) | num_sublayers(3 bits = 1) | constant_frame_rate(2 bits = 1) | chroma_format_idc(2 bits = 1 => 4:2:0)
    let word1: u16 = (1 << 4) | (1 << 2) | 1;
    rec.extend_from_slice(&word1.to_be_bytes());
    // Byte 3: bit_depth_minus8(3 bits = 0) | reserved('11111'b)
    rec.push(0b0001_1111);

    // VvcPTLRecord(1):
    // reserved('00'b) | num_bytes_constraint_info(6 bits = 1)
    rec.push(0x01);
    // general_profile_idc(7 bits = 1 Main 10) | general_tier_flag(1 bit = 0)
    rec.push(1 << 1);
    // general_level_idc(8 bits)
    rec.push(level_idc);
    // ptl_frame_only_constraint_flag(1 = 1) | ptl_multi_layer_enabled_flag(1 = 0) | general_constraint_info(6 bits = 0)
    rec.push(0x80);
    // num_sub_profiles(8 bits = 0)
    rec.push(0x00);

    // max_picture_width (u16), max_picture_height (u16), avg_frame_rate (u16 = 0)
    rec.extend_from_slice(&(width.min(u16::MAX as u32) as u16).to_be_bytes());
    rec.extend_from_slice(&(height.min(u16::MAX as u32) as u16).to_be_bytes());
    rec.extend_from_slice(&0u16.to_be_bytes());

    let mut arrays: Vec<(u8, &[u8])> = Vec::new();
    if let Some(opi) = params.opi.as_deref() {
        arrays.push((12, opi));
    }
    if let Some(dci) = params.dci.as_deref() {
        arrays.push((13, dci));
    }
    if let Some(vps) = params.vps.as_deref() {
        arrays.push((14, vps));
    }
    if let Some(sps) = params.sps.as_deref() {
        arrays.push((15, sps));
    }
    if let Some(pps) = params.pps.as_deref() {
        arrays.push((16, pps));
    }

    rec.push(arrays.len() as u8);
    for (nal_type, nalu) in arrays {
        // array_completeness(1 = 1) | reserved(2 = 0) | NAL_unit_type(5)
        rec.push(0x80 | (nal_type & 0x1F));
        if nal_type != 13 && nal_type != 12 {
            rec.extend_from_slice(&1u16.to_be_bytes()); // num_nalus = 1
        }
        rec.extend_from_slice(&(nalu.len() as u16).to_be_bytes());
        rec.extend_from_slice(nalu);
    }

    rec
}

struct VvcVideoSample {
    data: Vec<u8>,
    duration: u32,
    keyframe: bool,
}

struct FlacAudioSample {
    data: Vec<u8>,
    samples: u32,
}

/// Self-contained ISO-BMFF `.mp4` writer for H.266 / VVC (`vvc1` + `vvcC`) and optional FLAC (`fLaC` + `dfLa`) audio.
pub struct VvcMp4Muxer<W: Write> {
    sink: W,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    audio: Option<(AudioSpec, u32, u16)>,
    params: VvcParamSets,
    video_samples: Vec<VvcVideoSample>,
    audio_samples: Vec<FlacAudioSample>,
}

impl<W: Write> VvcMp4Muxer<W> {
    pub fn new(
        sink: W,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        audio: Option<(AudioSpec, u32, u16)>,
    ) -> Self {
        Self {
            sink,
            width,
            height,
            fps_num: fps_num.max(1),
            fps_den: fps_den.max(1),
            audio,
            params: VvcParamSets::default(),
            video_samples: Vec::new(),
            audio_samples: Vec::new(),
        }
    }

    pub fn push_video(&mut self, data: &[u8], keyframe: bool) {
        let (extracted, length_prefixed) = annexb_to_vvcc_packets(data);
        if self.params.opi.is_none() {
            self.params.opi = extracted.opi;
        }
        if self.params.dci.is_none() {
            self.params.dci = extracted.dci;
        }
        if self.params.vps.is_none() {
            self.params.vps = extracted.vps;
        }
        if self.params.sps.is_none() {
            self.params.sps = extracted.sps;
        }
        if self.params.pps.is_none() {
            self.params.pps = extracted.pps;
        }
        let payload = if length_prefixed.is_empty() {
            data.to_vec()
        } else {
            length_prefixed
        };
        self.video_samples.push(VvcVideoSample {
            data: payload,
            duration: frame_duration(self.fps_num, self.fps_den),
            keyframe,
        });
    }

    pub fn push_audio(&mut self, data: &[u8], samples: u32) -> Result<(), MuxError> {
        if self.audio.is_none() {
            return Err(MuxError::BadAudioConfig(
                "this muxer was created without an audio track".to_string(),
            ));
        }
        if samples == 0 {
            return Err(MuxError::BadAudioConfig(
                "an audio frame must contain at least one sample".to_string(),
            ));
        }
        self.audio_samples.push(FlacAudioSample {
            data: data.to_vec(),
            samples,
        });
        Ok(())
    }

    pub fn finish(mut self) -> Result<W, MuxError> {
        // 1. Build `ftyp` box
        let mut ftyp_payload = Vec::new();
        ftyp_payload.extend_from_slice(b"isom"); // major_brand
        ftyp_payload.extend_from_slice(&512u32.to_be_bytes()); // minor_version
        ftyp_payload.extend_from_slice(b"isomiso2mp41vvc1"); // compatible_brands
        let ftyp_box = make_box(b"ftyp", &ftyp_payload);

        // 2. Compute `mdat` layout: `ftyp` is followed immediately by `mdat`, then `moov`.
        let mdat_header_offset = ftyp_box.len() as u64;
        let mdat_data_offset = mdat_header_offset + 8;

        let mut mdat_payload = Vec::new();
        let mut video_offsets = Vec::with_capacity(self.video_samples.len());
        let mut video_sizes = Vec::with_capacity(self.video_samples.len());
        let mut sync_samples = Vec::new();

        for (idx, sample) in self.video_samples.iter().enumerate() {
            let offset = mdat_data_offset + (mdat_payload.len() as u64);
            video_offsets.push(offset);
            video_sizes.push(sample.data.len() as u32);
            if sample.keyframe {
                sync_samples.push((idx + 1) as u32);
            }
            mdat_payload.extend_from_slice(&sample.data);
        }

        let mut audio_offsets = Vec::with_capacity(self.audio_samples.len());
        let mut audio_sizes = Vec::with_capacity(self.audio_samples.len());
        for sample in &self.audio_samples {
            let offset = mdat_data_offset + (mdat_payload.len() as u64);
            audio_offsets.push(offset);
            audio_sizes.push(sample.data.len() as u32);
            mdat_payload.extend_from_slice(&sample.data);
        }

        let mdat_box = make_box(b"mdat", &mdat_payload);

        // 3. Build `moov` box
        let total_video_ticks: u64 = self
            .video_samples
            .iter()
            .map(|s| u64::from(s.duration))
            .sum();
        let mvhd_box = build_mvhd_box(
            VIDEO_TIMESCALE as u32,
            total_video_ticks,
            if self.audio.is_some() { 3 } else { 2 },
        );

        let vvcc_record = build_vvcc_record(&self.params, self.width, self.height);
        let video_trak = build_vvc_video_trak(
            self.width,
            self.height,
            VIDEO_TIMESCALE as u32,
            total_video_ticks,
            frame_duration(self.fps_num, self.fps_den),
            &vvcc_record,
            &video_sizes,
            &video_offsets,
            &sync_samples,
        );

        let mut moov_payload = Vec::new();
        moov_payload.extend_from_slice(&mvhd_box);
        moov_payload.extend_from_slice(&video_trak);

        if let Some((AudioSpec::Flac { streaminfo }, sample_rate, channels)) = &self.audio {
            if !self.audio_samples.is_empty() {
                let total_audio_samples: u64 =
                    self.audio_samples.iter().map(|s| u64::from(s.samples)).sum();
                let frame_samples = self.audio_samples[0].samples;
                let audio_trak = build_flac_audio_trak(
                    *sample_rate,
                    *channels,
                    total_audio_samples,
                    frame_samples,
                    streaminfo,
                    &audio_sizes,
                    &audio_offsets,
                );
                moov_payload.extend_from_slice(&audio_trak);
            }
        }

        let moov_box = make_box(b"moov", &moov_payload);

        self.sink.write_all(&ftyp_box)?;
        self.sink.write_all(&mdat_box)?;
        self.sink.write_all(&moov_box)?;
        self.sink.flush()?;
        Ok(self.sink)
    }
}

fn make_box(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let total = (8 + payload.len()) as u32;
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(payload);
    out
}

fn make_full_box(fourcc: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let total = (12 + payload.len()) as u32;
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(fourcc);
    let vf = (u32::from(version) << 24) | (flags & 0x00FF_FFFF);
    out.extend_from_slice(&vf.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn build_mvhd_box(timescale: u32, duration: u64, next_track_id: u32) -> Vec<u8> {
    let mut p = Vec::with_capacity(96);
    p.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    p.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    p.extend_from_slice(&timescale.to_be_bytes());
    p.extend_from_slice(&(duration.min(u32::MAX as u64) as u32).to_be_bytes());
    p.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate = 1.0
    p.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
    p.extend_from_slice(&[0u8; 10]); // reserved
    // Unity matrix
    for &v in &[
        0x0001_0000u32,
        0,
        0,
        0,
        0x0001_0000,
        0,
        0,
        0,
        0x4000_0000,
    ] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(&[0u8; 24]); // pre_defined
    p.extend_from_slice(&next_track_id.to_be_bytes());
    make_full_box(b"mvhd", 0, 0, &p)
}

#[allow(clippy::too_many_arguments)]
fn build_vvc_video_trak(
    width: u32,
    height: u32,
    timescale: u32,
    duration: u64,
    sample_delta: u32,
    vvcc_record: &[u8],
    sizes: &[u32],
    offsets: &[u64],
    sync_samples: &[u32],
) -> Vec<u8> {
    // tkhd (flags = 3: track_enabled | track_in_movie)
    let mut tkhd_p = Vec::with_capacity(80);
    tkhd_p.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    tkhd_p.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    tkhd_p.extend_from_slice(&1u32.to_be_bytes()); // track_ID = 1
    tkhd_p.extend_from_slice(&0u32.to_be_bytes()); // reserved
    tkhd_p.extend_from_slice(&(duration.min(u32::MAX as u64) as u32).to_be_bytes());
    tkhd_p.extend_from_slice(&[0u8; 8]); // reserved
    tkhd_p.extend_from_slice(&0u16.to_be_bytes()); // layer
    tkhd_p.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
    tkhd_p.extend_from_slice(&0u16.to_be_bytes()); // volume = 0 for video
    tkhd_p.extend_from_slice(&0u16.to_be_bytes()); // reserved
    for &v in &[
        0x0001_0000u32,
        0,
        0,
        0,
        0x0001_0000,
        0,
        0,
        0,
        0x4000_0000,
    ] {
        tkhd_p.extend_from_slice(&v.to_be_bytes());
    }
    tkhd_p.extend_from_slice(&(width << 16).to_be_bytes());
    tkhd_p.extend_from_slice(&(height << 16).to_be_bytes());
    let tkhd = make_full_box(b"tkhd", 0, 3, &tkhd_p);

    // mdhd
    let mut mdhd_p = Vec::with_capacity(20);
    mdhd_p.extend_from_slice(&0u32.to_be_bytes());
    mdhd_p.extend_from_slice(&0u32.to_be_bytes());
    mdhd_p.extend_from_slice(&timescale.to_be_bytes());
    mdhd_p.extend_from_slice(&(duration.min(u32::MAX as u64) as u32).to_be_bytes());
    mdhd_p.extend_from_slice(&0x55C4u16.to_be_bytes()); // 'und'
    mdhd_p.extend_from_slice(&0u16.to_be_bytes());
    let mdhd = make_full_box(b"mdhd", 0, 0, &mdhd_p);

    // hdlr ('vide')
    let mut hdlr_p = Vec::with_capacity(32);
    hdlr_p.extend_from_slice(&0u32.to_be_bytes());
    hdlr_p.extend_from_slice(b"vide");
    hdlr_p.extend_from_slice(&[0u8; 12]);
    hdlr_p.extend_from_slice(b"Kine VVC Video\0");
    let hdlr = make_full_box(b"hdlr", 0, 0, &hdlr_p);

    // vmhd (flags = 1)
    let vmhd = make_full_box(b"vmhd", 0, 1, &[0u8; 8]);

    // dinf -> dref -> url (flags = 1)
    let url = make_full_box(b"url ", 0, 1, &[]);
    let mut dref_p = Vec::with_capacity(4 + url.len());
    dref_p.extend_from_slice(&1u32.to_be_bytes());
    dref_p.extend_from_slice(&url);
    let dref = make_full_box(b"dref", 0, 0, &dref_p);
    let dinf = make_box(b"dinf", &dref);

    // stsd -> vvc1 -> vvcC (+ colr nclx BT.709)
    let vvcc_box = make_full_box(b"vvcC", 0, 0, vvcc_record);
    let mut colr_p = Vec::with_capacity(11);
    colr_p.extend_from_slice(b"nclx");
    colr_p.extend_from_slice(&1u16.to_be_bytes()); // BT.709 primaries
    colr_p.extend_from_slice(&1u16.to_be_bytes()); // BT.709 transfer
    colr_p.extend_from_slice(&1u16.to_be_bytes()); // BT.709 matrix
    colr_p.push(0x00); // limited range
    let colr_box = make_box(b"colr", &colr_p);

    let mut vvc1_p = Vec::with_capacity(78 + vvcc_box.len() + colr_box.len());
    vvc1_p.extend_from_slice(&[0u8; 6]); // reserved
    vvc1_p.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index = 1
    vvc1_p.extend_from_slice(&[0u8; 16]); // pre_defined + reserved
    vvc1_p.extend_from_slice(&(width as u16).to_be_bytes());
    vvc1_p.extend_from_slice(&(height as u16).to_be_bytes());
    vvc1_p.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // horizresolution = 72 dpi
    vvc1_p.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // vertresolution = 72 dpi
    vvc1_p.extend_from_slice(&0u32.to_be_bytes()); // reserved
    vvc1_p.extend_from_slice(&1u16.to_be_bytes()); // frame_count = 1
    let mut compressor = [0u8; 32];
    let label = b"Kine H.266/VVC";
    compressor[0] = label.len() as u8;
    compressor[1..1 + label.len()].copy_from_slice(label);
    vvc1_p.extend_from_slice(&compressor);
    vvc1_p.extend_from_slice(&0x0018u16.to_be_bytes()); // depth = 24
    vvc1_p.extend_from_slice(&(-1i16).to_be_bytes()); // pre_defined = -1
    vvc1_p.extend_from_slice(&vvcc_box);
    vvc1_p.extend_from_slice(&colr_box);
    let vvc1_box = make_box(b"vvc1", &vvc1_p);

    let mut stsd_p = Vec::with_capacity(4 + vvc1_box.len());
    stsd_p.extend_from_slice(&1u32.to_be_bytes());
    stsd_p.extend_from_slice(&vvc1_box);
    let stsd = make_full_box(b"stsd", 0, 0, &stsd_p);

    // stts
    let mut stts_p = Vec::with_capacity(12);
    stts_p.extend_from_slice(&1u32.to_be_bytes());
    stts_p.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
    stts_p.extend_from_slice(&sample_delta.to_be_bytes());
    let stts = make_full_box(b"stts", 0, 0, &stts_p);

    // stss
    let mut stss_p = Vec::with_capacity(4 + sync_samples.len() * 4);
    stss_p.extend_from_slice(&(sync_samples.len() as u32).to_be_bytes());
    for &s in sync_samples {
        stss_p.extend_from_slice(&s.to_be_bytes());
    }
    let stss = make_full_box(b"stss", 0, 0, &stss_p);

    // stsc (1 sample per chunk)
    let mut stsc_p = Vec::with_capacity(16);
    stsc_p.extend_from_slice(&1u32.to_be_bytes());
    stsc_p.extend_from_slice(&1u32.to_be_bytes()); // first_chunk = 1
    stsc_p.extend_from_slice(&1u32.to_be_bytes()); // samples_per_chunk = 1
    stsc_p.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index = 1
    let stsc = make_full_box(b"stsc", 0, 0, &stsc_p);

    // stsz
    let mut stsz_p = Vec::with_capacity(8 + sizes.len() * 4);
    stsz_p.extend_from_slice(&0u32.to_be_bytes()); // sample_size = 0 (variable)
    stsz_p.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
    for &sz in sizes {
        stsz_p.extend_from_slice(&sz.to_be_bytes());
    }
    let stsz = make_full_box(b"stsz", 0, 0, &stsz_p);

    // co64
    let mut co64_p = Vec::with_capacity(4 + offsets.len() * 8);
    co64_p.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for &off in offsets {
        co64_p.extend_from_slice(&off.to_be_bytes());
    }
    let co64 = make_full_box(b"co64", 0, 0, &co64_p);

    let mut stbl_p = Vec::new();
    stbl_p.extend_from_slice(&stsd);
    stbl_p.extend_from_slice(&stts);
    stbl_p.extend_from_slice(&stss);
    stbl_p.extend_from_slice(&stsc);
    stbl_p.extend_from_slice(&stsz);
    stbl_p.extend_from_slice(&co64);
    let stbl = make_box(b"stbl", &stbl_p);

    let mut minf_p = Vec::new();
    minf_p.extend_from_slice(&vmhd);
    minf_p.extend_from_slice(&dinf);
    minf_p.extend_from_slice(&stbl);
    let minf = make_box(b"minf", &minf_p);

    let mut mdia_p = Vec::new();
    mdia_p.extend_from_slice(&mdhd);
    mdia_p.extend_from_slice(&hdlr);
    mdia_p.extend_from_slice(&minf);
    let mdia = make_box(b"mdia", &mdia_p);

    let mut trak_p = Vec::new();
    trak_p.extend_from_slice(&tkhd);
    trak_p.extend_from_slice(&mdia);
    make_box(b"trak", &trak_p)
}

fn build_flac_audio_trak(
    sample_rate: u32,
    channels: u16,
    total_samples: u64,
    frame_samples: u32,
    streaminfo: &[u8],
    sizes: &[u32],
    offsets: &[u64],
) -> Vec<u8> {
    let mut tkhd_p = Vec::with_capacity(80);
    tkhd_p.extend_from_slice(&0u32.to_be_bytes());
    tkhd_p.extend_from_slice(&0u32.to_be_bytes());
    tkhd_p.extend_from_slice(&2u32.to_be_bytes()); // track_ID = 2
    tkhd_p.extend_from_slice(&0u32.to_be_bytes());
    tkhd_p.extend_from_slice(&(total_samples.min(u32::MAX as u64) as u32).to_be_bytes());
    tkhd_p.extend_from_slice(&[0u8; 8]);
    tkhd_p.extend_from_slice(&0u16.to_be_bytes());
    tkhd_p.extend_from_slice(&0u16.to_be_bytes());
    tkhd_p.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
    tkhd_p.extend_from_slice(&0u16.to_be_bytes());
    for &v in &[
        0x0001_0000u32,
        0,
        0,
        0,
        0x0001_0000,
        0,
        0,
        0,
        0x4000_0000,
    ] {
        tkhd_p.extend_from_slice(&v.to_be_bytes());
    }
    tkhd_p.extend_from_slice(&0u32.to_be_bytes());
    tkhd_p.extend_from_slice(&0u32.to_be_bytes());
    let tkhd = make_full_box(b"tkhd", 0, 3, &tkhd_p);

    let mut mdhd_p = Vec::with_capacity(20);
    mdhd_p.extend_from_slice(&0u32.to_be_bytes());
    mdhd_p.extend_from_slice(&0u32.to_be_bytes());
    mdhd_p.extend_from_slice(&sample_rate.to_be_bytes());
    mdhd_p.extend_from_slice(&(total_samples.min(u32::MAX as u64) as u32).to_be_bytes());
    mdhd_p.extend_from_slice(&0x55C4u16.to_be_bytes());
    mdhd_p.extend_from_slice(&0u16.to_be_bytes());
    let mdhd = make_full_box(b"mdhd", 0, 0, &mdhd_p);

    let mut hdlr_p = Vec::with_capacity(32);
    hdlr_p.extend_from_slice(&0u32.to_be_bytes());
    hdlr_p.extend_from_slice(b"soun");
    hdlr_p.extend_from_slice(&[0u8; 12]);
    hdlr_p.extend_from_slice(b"Kine FLAC Audio\0");
    let hdlr = make_full_box(b"hdlr", 0, 0, &hdlr_p);

    let smhd = make_full_box(b"smhd", 0, 0, &[0u8; 4]);

    let url = make_full_box(b"url ", 0, 1, &[]);
    let mut dref_p = Vec::with_capacity(4 + url.len());
    dref_p.extend_from_slice(&1u32.to_be_bytes());
    dref_p.extend_from_slice(&url);
    let dref = make_full_box(b"dref", 0, 0, &dref_p);
    let dinf = make_box(b"dinf", &dref);

    // dfLa box inside fLaC AudioSampleEntry
    let mut dfla_p = Vec::with_capacity(4 + streaminfo.len());
    dfla_p.push(0x80); // last_metadata_block_flag(1) | BLOCK_TYPE_STREAMINFO(0)
    dfla_p.extend_from_slice(&[0, 0, streaminfo.len() as u8]);
    dfla_p.extend_from_slice(streaminfo);
    let dfla_box = make_full_box(b"dfLa", 0, 0, &dfla_p);

    let mut flac_p = Vec::with_capacity(28 + dfla_box.len());
    flac_p.extend_from_slice(&[0u8; 6]);
    flac_p.extend_from_slice(&1u16.to_be_bytes());
    flac_p.extend_from_slice(&[0u8; 8]);
    flac_p.extend_from_slice(&channels.to_be_bytes());
    flac_p.extend_from_slice(&16u16.to_be_bytes());
    flac_p.extend_from_slice(&0u16.to_be_bytes());
    flac_p.extend_from_slice(&0u16.to_be_bytes());
    flac_p.extend_from_slice(&(sample_rate << 16).to_be_bytes());
    flac_p.extend_from_slice(&dfla_box);
    let flac_box = make_box(b"fLaC", &flac_p);

    let mut stsd_p = Vec::with_capacity(4 + flac_box.len());
    stsd_p.extend_from_slice(&1u32.to_be_bytes());
    stsd_p.extend_from_slice(&flac_box);
    let stsd = make_full_box(b"stsd", 0, 0, &stsd_p);

    let mut stts_p = Vec::with_capacity(12);
    stts_p.extend_from_slice(&1u32.to_be_bytes());
    stts_p.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
    stts_p.extend_from_slice(&frame_samples.to_be_bytes());
    let stts = make_full_box(b"stts", 0, 0, &stts_p);

    let mut stsc_p = Vec::with_capacity(16);
    stsc_p.extend_from_slice(&1u32.to_be_bytes());
    stsc_p.extend_from_slice(&1u32.to_be_bytes());
    stsc_p.extend_from_slice(&1u32.to_be_bytes());
    stsc_p.extend_from_slice(&1u32.to_be_bytes());
    let stsc = make_full_box(b"stsc", 0, 0, &stsc_p);

    let mut stsz_p = Vec::with_capacity(8 + sizes.len() * 4);
    stsz_p.extend_from_slice(&0u32.to_be_bytes());
    stsz_p.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
    for &sz in sizes {
        stsz_p.extend_from_slice(&sz.to_be_bytes());
    }
    let stsz = make_full_box(b"stsz", 0, 0, &stsz_p);

    let mut co64_p = Vec::with_capacity(4 + offsets.len() * 8);
    co64_p.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for &off in offsets {
        co64_p.extend_from_slice(&off.to_be_bytes());
    }
    let co64 = make_full_box(b"co64", 0, 0, &co64_p);

    let mut stbl_p = Vec::new();
    stbl_p.extend_from_slice(&stsd);
    stbl_p.extend_from_slice(&stts);
    stbl_p.extend_from_slice(&stsc);
    stbl_p.extend_from_slice(&stsz);
    stbl_p.extend_from_slice(&co64);
    let stbl = make_box(b"stbl", &stbl_p);

    let mut minf_p = Vec::new();
    minf_p.extend_from_slice(&smhd);
    minf_p.extend_from_slice(&dinf);
    minf_p.extend_from_slice(&stbl);
    let minf = make_box(b"minf", &minf_p);

    let mut mdia_p = Vec::new();
    mdia_p.extend_from_slice(&mdhd);
    mdia_p.extend_from_slice(&hdlr);
    mdia_p.extend_from_slice(&minf);
    let mdia = make_box(b"mdia", &mdia_p);

    let mut trak_p = Vec::new();
    trak_p.extend_from_slice(&tkhd);
    trak_p.extend_from_slice(&mdia);
    make_box(b"trak", &trak_p)
}
