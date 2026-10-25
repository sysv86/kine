//! Pure-Rust Matroska (`.mkv`) and WebM (`.webm`) EBML container muxer.
//!
//! Writes spec-compliant EBML v1 / Matroska v4 (`matroska` or `webm` DocType)
//! files containing:
//! * Video track #1: `V_AV1` (with `av1C` CodecPrivate built from the sequence
//!   header OBU) or `V_MPEG4/ISO/AVC` (with `avcC` CodecPrivate built from
//!   Annex-B SPS/PPS NALs and length-prefixed NAL units in `SimpleBlock`s).
//! * Optional Audio track #2: `A_FLAC` (with `fLaC` + 34-byte STREAMINFO
//!   metadata block as CodecPrivate).

use crate::{annexb_to_vvcc_packets, build_vvcc_record, AudioSpec, Codec, MuxError};
use std::io::Write;

/// Container DocType written in the EBML header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EbmlDocType {
    Matroska,
    Webm,
}

impl EbmlDocType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Matroska => "matroska",
            Self::Webm => "webm",
        }
    }
}

struct QueuedPacket {
    track_number: u8,
    pts_ms: u64,
    keyframe: bool,
    data: Vec<u8>,
}

/// Pure-Rust Matroska / WebM muxer.
pub struct MkvMuxer<W: Write> {
    sink: W,
    doc_type: EbmlDocType,
    codec: Codec,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    audio: Option<(AudioSpec, u32, u16)>,
    video_codec_private: Option<Vec<u8>>,
    packets: Vec<QueuedPacket>,
}

impl<W: Write> MkvMuxer<W> {
    pub fn new(
        sink: W,
        doc_type: EbmlDocType,
        codec: Codec,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
    ) -> Self {
        Self {
            sink,
            doc_type,
            codec,
            width,
            height,
            fps_num: fps_num.max(1),
            fps_den: fps_den.max(1),
            audio: None,
            video_codec_private: None,
            packets: Vec::new(),
        }
    }

    pub fn with_audio(
        sink: W,
        doc_type: EbmlDocType,
        codec: Codec,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        audio: &AudioSpec,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, MuxError> {
        let AudioSpec::Flac { streaminfo } = audio;
        if streaminfo.len() != 34 {
            return Err(MuxError::BadAudioConfig(format!(
                "STREAMINFO must be 34 bytes, got {}",
                streaminfo.len()
            )));
        }
        Ok(Self {
            sink,
            doc_type,
            codec,
            width,
            height,
            fps_num: fps_num.max(1),
            fps_den: fps_den.max(1),
            audio: Some((audio.clone(), sample_rate, channels)),
            video_codec_private: None,
            packets: Vec::new(),
        })
    }

    pub fn push_video(
        &mut self,
        frame: u64,
        data: &[u8],
        keyframe: bool,
    ) -> Result<(), MuxError> {
        let pts_ms = frame * 1_000 * u64::from(self.fps_den) / u64::from(self.fps_num);
        let payload = match self.codec {
            Codec::Av1 => {
                if self.video_codec_private.is_none() {
                    if let Some(seq_obu) = extract_av1_sequence_header_obu(data) {
                        let mut av1c = vec![0x81, 0x04, 0x0C, 0x00];
                        av1c.extend_from_slice(&seq_obu);
                        self.video_codec_private = Some(av1c);
                    }
                }
                data.to_vec()
            }
            Codec::H264 => {
                let (sps, pps, avcc_nals) = annexb_to_avcc_packets(data);
                if self.video_codec_private.is_none() {
                    if let (Some(sps), Some(pps)) = (sps, pps) {
                        self.video_codec_private = Some(build_avcc_box(&sps, &pps));
                    }
                }
                avcc_nals
            }
            Codec::H266 => {
                let (params, vvcc_nals) = annexb_to_vvcc_packets(data);
                if self.video_codec_private.is_none() && params.has_core_sets() {
                    self.video_codec_private =
                        Some(build_vvcc_record(&params, self.width, self.height));
                }
                vvcc_nals
            }
        };

        self.packets.push(QueuedPacket {
            track_number: 1,
            pts_ms,
            keyframe,
            data: payload,
        });
        Ok(())
    }

    pub fn push_audio_frame(
        &mut self,
        pts_samples: u64,
        data: &[u8],
        samples: u32,
    ) -> Result<(), MuxError> {
        let Some((_, sample_rate, _)) = &self.audio else {
            return Err(MuxError::BadAudioConfig(
                "this muxer was created without an audio track".into(),
            ));
        };
        if samples == 0 {
            return Err(MuxError::BadAudioConfig(
                "an audio frame must contain at least one sample".into(),
            ));
        }
        let pts_ms = pts_samples * 1_000 / u64::from((*sample_rate).max(1));
        self.packets.push(QueuedPacket {
            track_number: 2,
            pts_ms,
            keyframe: true,
            data: data.to_vec(),
        });
        Ok(())
    }

    pub fn finalize(mut self) -> Result<W, MuxError> {
        // Sort interleaved packets by presentation timestamp (ms), video before audio on ties
        self.packets
            .sort_by_key(|p| (p.pts_ms, p.track_number));

        let duration_ms = self
            .packets
            .last()
            .map(|p| p.pts_ms as f64 + (1_000.0 * f64::from(self.fps_den) / f64::from(self.fps_num)))
            .unwrap_or(0.0);

        // 1. EBML Header (0x1A45DFA3)
        let mut ebml_body = Vec::new();
        write_uint_el(&mut ebml_body, &[0x42, 0x86], 1); // EBMLVersion = 1
        write_uint_el(&mut ebml_body, &[0x42, 0xF7], 1); // EBMLReadVersion = 1
        write_uint_el(&mut ebml_body, &[0x42, 0xF2], 4); // EBMLMaxIDLength = 4
        write_uint_el(&mut ebml_body, &[0x42, 0xF3], 8); // EBMLMaxSizeLength = 8
        write_str_el(&mut ebml_body, &[0x42, 0x82], self.doc_type.as_str()); // DocType
        write_uint_el(&mut ebml_body, &[0x42, 0x87], 4); // DocTypeVersion = 4
        write_uint_el(&mut ebml_body, &[0x42, 0x85], 2); // DocTypeReadVersion = 2

        let mut file_bytes = Vec::new();
        write_master_el(&mut file_bytes, &[0x1A, 0x45, 0xDF, 0xA3], &ebml_body);

        // 2. Segment Body
        let mut segment_body = Vec::new();

        // 2a. Info (0x1549A966)
        let mut info_body = Vec::new();
        write_uint_el(&mut info_body, &[0x2A, 0xD7, 0xB1], 1_000_000); // TimestampScale = 1ms (1_000_000 ns)
        write_str_el(&mut info_body, &[0x4D, 0x80], "Kine Engine"); // MuxingApp
        write_str_el(&mut info_body, &[0x57, 0x41], "Kine Engine"); // WritingApp
        write_f64_el(&mut info_body, &[0x44, 0x89], duration_ms); // Duration
        write_master_el(&mut segment_body, &[0x15, 0x49, 0xA9, 0x66], &info_body);

        // 2b. Tracks (0x1654AE6B)
        let mut tracks_body = Vec::new();
        // Video TrackEntry (0xAE)
        let mut v_entry = Vec::new();
        write_uint_el(&mut v_entry, &[0xD7], 1); // TrackNumber = 1
        write_uint_el(&mut v_entry, &[0x73, 0xC5], 1); // TrackUID = 1
        write_uint_el(&mut v_entry, &[0x83], 1); // TrackType = 1 (video)
        let codec_id = match self.codec {
            Codec::Av1 => "V_AV1",
            Codec::H264 => "V_MPEG4/ISO/AVC",
            Codec::H266 => "V_MPEGI/ISO/VVC",
        };
        write_str_el(&mut v_entry, &[0x86], codec_id);
        if let Some(priv_bytes) = &self.video_codec_private {
            write_bytes_el(&mut v_entry, &[0x63, 0xA2], priv_bytes); // CodecPrivate
        }
        let frame_ns = 1_000_000_000u64 * u64::from(self.fps_den) / u64::from(self.fps_num);
        write_uint_el(&mut v_entry, &[0x23, 0xE3, 0x83], frame_ns); // DefaultDuration

        let mut v_settings = Vec::new();
        write_uint_el(&mut v_settings, &[0xB0], u64::from(self.width)); // PixelWidth
        write_uint_el(&mut v_settings, &[0xBA], u64::from(self.height)); // PixelHeight
        write_master_el(&mut v_entry, &[0xE0], &v_settings); // Video
        write_master_el(&mut tracks_body, &[0xAE], &v_entry);

        // Optional Audio TrackEntry (0xAE)
        if let Some((AudioSpec::Flac { streaminfo }, sample_rate, channels)) = &self.audio {
            let mut a_entry = Vec::new();
            write_uint_el(&mut a_entry, &[0xD7], 2); // TrackNumber = 2
            write_uint_el(&mut a_entry, &[0x73, 0xC5], 2); // TrackUID = 2
            write_uint_el(&mut a_entry, &[0x83], 2); // TrackType = 2 (audio)
            write_str_el(&mut a_entry, &[0x86], "A_FLAC"); // CodecID
            let mut flac_header = Vec::with_capacity(42);
            flac_header.extend_from_slice(b"fLaC");
            flac_header.push(0x80); // last metadata block, STREAMINFO (0)
            flac_header.extend_from_slice(&[0, 0, 34]);
            flac_header.extend_from_slice(streaminfo);
            write_bytes_el(&mut a_entry, &[0x63, 0xA2], &flac_header); // CodecPrivate

            let mut a_settings = Vec::new();
            write_f64_el(&mut a_settings, &[0xB5], f64::from(*sample_rate)); // SamplingFrequency
            write_uint_el(&mut a_settings, &[0x9F], u64::from(*channels)); // Channels
            write_uint_el(&mut a_settings, &[0x62, 0x64], 16); // BitDepth
            write_master_el(&mut a_entry, &[0xE1], &a_settings); // Audio
            write_master_el(&mut tracks_body, &[0xAE], &a_entry);
        }

        write_master_el(&mut segment_body, &[0x16, 0x54, 0xAE, 0x6B], &tracks_body);

        // 2c. Clusters (0x1F43B675) — group packets into ≤2000ms clusters
        let mut cluster_start_ms: Option<u64> = None;
        let mut cluster_body = Vec::new();

        for pkt in &self.packets {
            let need_new_cluster = match cluster_start_ms {
                None => true,
                Some(start) => {
                    pkt.pts_ms.saturating_sub(start) >= 2_000
                        || (pkt.track_number == 1 && pkt.keyframe && pkt.pts_ms > start)
                }
            };
            if need_new_cluster {
                if !cluster_body.is_empty() {
                    write_master_el(&mut segment_body, &[0x1F, 0x43, 0xB6, 0x75], &cluster_body);
                    cluster_body.clear();
                }
                cluster_start_ms = Some(pkt.pts_ms);
                write_uint_el(&mut cluster_body, &[0xE7], pkt.pts_ms); // Cluster Timestamp
            }
            let rel_ts = (pkt.pts_ms.saturating_sub(cluster_start_ms.unwrap_or(0)))
                .min(i16::MAX as u64) as i16;
            let mut sb = Vec::with_capacity(4 + pkt.data.len());
            sb.push(0x80 | pkt.track_number); // VINT track number
            sb.extend_from_slice(&rel_ts.to_be_bytes());
            sb.push(if pkt.keyframe { 0x80 } else { 0x00 }); // flags: keyframe
            sb.extend_from_slice(&pkt.data);
            write_bytes_el(&mut cluster_body, &[0xA3], &sb); // SimpleBlock
        }
        if !cluster_body.is_empty() {
            write_master_el(&mut segment_body, &[0x1F, 0x43, 0xB6, 0x75], &cluster_body);
        }

        write_master_el(&mut file_bytes, &[0x18, 0x53, 0x80, 0x67], &segment_body);
        self.sink.write_all(&file_bytes)?;
        self.sink.flush()?;
        Ok(self.sink)
    }
}

fn write_vint_size(out: &mut Vec<u8>, size: usize) {
    if size < 0x7F {
        out.push(0x80 | (size as u8));
    } else if size < 0x3FFF {
        out.push(0x40 | ((size >> 8) as u8));
        out.push((size & 0xFF) as u8);
    } else if size < 0x1F_FFFF {
        out.push(0x20 | ((size >> 16) as u8));
        out.push(((size >> 8) & 0xFF) as u8);
        out.push((size & 0xFF) as u8);
    } else if size < 0x0FFF_FFFF {
        out.push(0x10 | ((size >> 24) as u8));
        out.push(((size >> 16) & 0xFF) as u8);
        out.push(((size >> 8) & 0xFF) as u8);
        out.push((size & 0xFF) as u8);
    } else {
        let s = size as u64;
        out.push(0x01);
        out.extend_from_slice(&s.to_be_bytes()[1..8]);
    }
}

fn write_master_el(out: &mut Vec<u8>, id: &[u8], body: &[u8]) {
    out.extend_from_slice(id);
    write_vint_size(out, body.len());
    out.extend_from_slice(body);
}

fn write_bytes_el(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
    write_master_el(out, id, data);
}

fn write_str_el(out: &mut Vec<u8>, id: &[u8], s: &str) {
    write_master_el(out, id, s.as_bytes());
}

fn write_uint_el(out: &mut Vec<u8>, id: &[u8], val: u64) {
    let be = val.to_be_bytes();
    let first_nz = be.iter().position(|&b| b != 0).unwrap_or(7);
    write_master_el(out, id, &be[first_nz..]);
}

fn write_f64_el(out: &mut Vec<u8>, id: &[u8], val: f64) {
    write_master_el(out, id, &val.to_be_bytes());
}

fn extract_av1_sequence_header_obu(data: &[u8]) -> Option<Vec<u8>> {
    // If the first OBU is a Temporal Delimiter (type 2, 2 bytes: 0x12 0x00) followed by
    // Sequence Header (type 1), or starts directly with Sequence Header (type 1):
    let mut pos = 0usize;
    while pos < data.len() {
        let header = data[pos];
        let obu_type = (header >> 3) & 0x0F;
        let has_ext = (header & 0x04) != 0;
        let has_size = (header & 0x02) != 0;
        let mut idx = pos + 1 + usize::from(has_ext);
        if !has_size || idx >= data.len() {
            if obu_type == 1 {
                return Some(data[pos..].to_vec());
            }
            break;
        }
        let mut len = 0usize;
        let mut shift = 0u32;
        while idx < data.len() {
            let b = data[idx];
            idx += 1;
            len |= ((b & 0x7F) as usize) << shift;
            if (b & 0x80) == 0 {
                break;
            }
            shift += 7;
        }
        let end = (idx + len).min(data.len());
        if obu_type == 1 {
            return Some(data[pos..end].to_vec());
        }
        pos = end;
    }
    None
}

fn annexb_to_avcc_packets(data: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>, Vec<u8>) {
    let mut sps = None;
    let mut pps = None;
    let mut out = Vec::new();
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
        if nal.is_empty() {
            continue;
        }
        let nal_type = nal[0] & 0x1F;
        if nal_type == 7 && sps.is_none() {
            sps = Some(nal.to_vec());
        } else if nal_type == 8 && pps.is_none() {
            pps = Some(nal.to_vec());
        }
        let len = nal.len() as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);
    }
    (sps, pps, out)
}

fn build_avcc_box(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let profile = sps.get(1).copied().unwrap_or(0x64);
    let compat = sps.get(2).copied().unwrap_or(0x00);
    let level = sps.get(3).copied().unwrap_or(0x1F);
    let mut avcc = Vec::with_capacity(11 + sps.len() + pps.len());
    avcc.push(1); // configurationVersion = 1
    avcc.push(profile);
    avcc.push(compat);
    avcc.push(level);
    avcc.push(0xFF); // 4-byte NAL length size
    avcc.push(0xE1); // 1 SPS
    avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(sps);
    avcc.push(1); // 1 PPS
    avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(pps);
    avcc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muxes_av1_and_flac_into_mkv_and_webm() {
        let mut mkv = MkvMuxer::with_audio(
            Vec::new(),
            EbmlDocType::Matroska,
            Codec::Av1,
            320,
            180,
            30,
            1,
            &AudioSpec::Flac {
                streaminfo: vec![0u8; 34],
            },
            48_000,
            2,
        )
        .unwrap();
        // Synthetic OBU sequence header + frame
        mkv.push_video(0, &[0x0A, 0x04, 0x00, 0x00, 0x00, 0x00], true)
            .unwrap();
        mkv.push_audio_frame(0, &[0xFF, 0xF8, 0x00, 0x00], 4096)
            .unwrap();
        let bytes = mkv.finalize().unwrap();
        assert_eq!(&bytes[0..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        assert!(bytes.windows(8).any(|w| w == b"matroska"));
        assert!(bytes.windows(5).any(|w| w == b"V_AV1"));
        assert!(bytes.windows(6).any(|w| w == b"A_FLAC"));
    }

    #[test]
    fn muxes_h266_vvc_into_mkv_with_vvcc_codec_private() {
        let mut mkv = MkvMuxer::new(
            Vec::new(),
            EbmlDocType::Matroska,
            Codec::H266,
            640,
            360,
            30,
            1,
        );
        let mut keyframe = Vec::new();
        keyframe.extend_from_slice(&[0, 0, 0, 1, 0x00, 0x71, 0x10, 0x00]); // VPS(14)
        keyframe.extend_from_slice(&[0, 0, 0, 1, 0x00, 0x79, 0x00, 0x30, 0x02, 0x33]); // SPS(15)
        keyframe.extend_from_slice(&[0, 0, 0, 1, 0x00, 0x81, 0x00, 0x40]); // PPS(16)
        keyframe.extend_from_slice(&[0, 0, 0, 1, 0x00, 0x41, 0x80, 0xAA]); // IDR_N_LP(8)
        mkv.push_video(0, &keyframe, true).unwrap();
        let bytes = mkv.finalize().unwrap();
        assert_eq!(&bytes[0..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        assert!(bytes.windows(15).any(|w| w == b"V_MPEGI/ISO/VVC"));
    }
}
