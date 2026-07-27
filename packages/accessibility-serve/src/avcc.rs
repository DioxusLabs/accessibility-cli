//! Annex-B to AVCC conversion for browser `VideoDecoder` clients.
//!
//! The encoder runs in Annex-B because that is what WebRTC's H.264 payloader
//! consumes. WebCodecs wants the other framing: 4-byte length prefixes plus a
//! one-time `avcC` record carrying the parameter sets. Converting here means a
//! single encoder can feed both transports at once.

/// Wire framing for the raw H.264 WebSocket transport.
///
/// `u32` big-endian length covering the tag byte and payload, then the tag,
/// then the payload.
pub mod tag {
    pub const PARAMETER_SET: u8 = 0x01;
    pub const KEYFRAME: u8 = 0x02;
    pub const DELTA: u8 = 0x03;
}

/// Frame a payload for the raw H.264 WebSocket transport.
pub fn envelope(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.extend_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
    out.push(tag);
    out.extend_from_slice(payload);
    out
}

/// Iterate the NAL units in an Annex-B stream.
///
/// Handles both 3- and 4-byte start codes, since VideoToolbox is consistent
/// but the parameter sets we prepend may not be.
pub fn nal_units(annex_b: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut index = 0usize;
    while index + 3 <= annex_b.len() {
        if annex_b[index] == 0 && annex_b[index + 1] == 0 {
            if annex_b[index + 2] == 1 {
                starts.push((index, 3));
                index += 3;
                continue;
            }
            if index + 4 <= annex_b.len() && annex_b[index + 2] == 0 && annex_b[index + 3] == 1 {
                starts.push((index, 4));
                index += 4;
                continue;
            }
        }
        index += 1;
    }

    let mut units = Vec::with_capacity(starts.len());
    for (position, (offset, code_len)) in starts.iter().enumerate() {
        let begin = offset + code_len;
        let end = starts
            .get(position + 1)
            .map(|(next, _)| *next)
            .unwrap_or(annex_b.len());
        if begin < end {
            units.push(&annex_b[begin..end]);
        }
    }
    units
}

/// Rewrite an Annex-B access unit as length-prefixed AVCC.
///
/// Parameter set NALs are dropped: in AVCC they belong in the `avcC` record,
/// not the bitstream.
pub fn to_avcc(annex_b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(annex_b.len());
    for unit in nal_units(annex_b) {
        if matches!(nal_type(unit), Some(7 | 8)) {
            continue;
        }
        out.extend_from_slice(&(unit.len() as u32).to_be_bytes());
        out.extend_from_slice(unit);
    }
    out
}

/// Pull the SPS (type 7) and PPS (type 8) out of an Annex-B access unit.
pub fn parameter_sets(annex_b: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut sps = None;
    let mut pps = None;
    for unit in nal_units(annex_b) {
        match nal_type(unit) {
            Some(7) if sps.is_none() => sps = Some(unit.to_vec()),
            Some(8) if pps.is_none() => pps = Some(unit.to_vec()),
            _ => {}
        }
    }
    let sps = sps?;
    let pps = pps?;
    // The avcC record copies profile/compatibility/level out of the SPS.
    (sps.len() >= 4).then_some((sps, pps))
}

fn nal_type(unit: &[u8]) -> Option<u8> {
    unit.first().map(|byte| byte & 0x1F)
}

/// Build an ISO/IEC 14496-15 `avcC` configuration record.
pub fn avcc_record(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sps.len() + pps.len() + 11);
    out.push(0x01); // configurationVersion
    out.extend_from_slice(&sps[1..4]); // profile, compatibility, level
    out.push(0xFF); // reserved | lengthSizeMinusOne = 3 (4-byte lengths)
    out.push(0xE1); // reserved | numOfSequenceParameterSets = 1
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(0x01); // numOfPictureParameterSets
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    out
}

/// The `codecs` string a browser needs to configure a decoder for this SPS.
pub fn codec_string(sps: &[u8]) -> String {
    format!("avc1.{:02X}{:02X}{:02X}", sps[1], sps[2], sps[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annex_b(units: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for unit in units {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(unit);
        }
        out
    }

    #[test]
    fn splits_four_byte_start_codes() {
        let stream = annex_b(&[&[0x67, 1, 2, 3], &[0x68, 9], &[0x65, 7, 7]]);
        let units = nal_units(&stream);
        assert_eq!(units.len(), 3);
        assert_eq!(units[0], &[0x67, 1, 2, 3]);
        assert_eq!(units[2], &[0x65, 7, 7]);
    }

    #[test]
    fn splits_three_byte_start_codes() {
        let stream = vec![0, 0, 1, 0x67, 1, 2, 3, 0, 0, 1, 0x65, 9];
        let units = nal_units(&stream);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0], &[0x67, 1, 2, 3]);
        assert_eq!(units[1], &[0x65, 9]);
    }

    #[test]
    fn avcc_drops_parameter_sets_and_prefixes_lengths() {
        let stream = annex_b(&[&[0x67, 1, 2, 3], &[0x68, 9], &[0x65, 7, 7]]);
        let avcc = to_avcc(&stream);
        // Only the IDR survives: 4 length bytes + 3 payload bytes.
        assert_eq!(avcc, vec![0, 0, 0, 3, 0x65, 7, 7]);
    }

    #[test]
    fn extracts_parameter_sets() {
        let stream = annex_b(&[&[0x67, 0x42, 0xC0, 0x1F], &[0x68, 9], &[0x65, 7]]);
        let (sps, pps) = parameter_sets(&stream).expect("parameter sets present");
        assert_eq!(sps, vec![0x67, 0x42, 0xC0, 0x1F]);
        assert_eq!(pps, vec![0x68, 9]);
        assert_eq!(codec_string(&sps), "avc1.42C01F");
    }

    #[test]
    fn parameter_sets_absent_on_delta_frames() {
        let stream = annex_b(&[&[0x41, 1, 2]]);
        assert!(parameter_sets(&stream).is_none());
    }

    #[test]
    fn avcc_record_layout() {
        let sps = [0x67, 0x42, 0xC0, 0x1F, 0xAA];
        let pps = [0x68, 0xCE];
        let record = avcc_record(&sps, &pps);
        assert_eq!(record[0], 1);
        assert_eq!(&record[1..4], &[0x42, 0xC0, 0x1F]);
        assert_eq!(record[4], 0xFF);
        assert_eq!(record[5], 0xE1);
        assert_eq!(&record[6..8], &(sps.len() as u16).to_be_bytes());
        assert_eq!(&record[8..8 + sps.len()], &sps);
    }

    #[test]
    fn envelope_length_covers_tag_and_payload() {
        let framed = envelope(tag::KEYFRAME, &[1, 2, 3]);
        assert_eq!(&framed[0..4], &4u32.to_be_bytes());
        assert_eq!(framed[4], tag::KEYFRAME);
        assert_eq!(&framed[5..], &[1, 2, 3]);
    }
}
