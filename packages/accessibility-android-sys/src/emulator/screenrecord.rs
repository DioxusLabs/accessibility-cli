use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use bytes::Bytes;

use crate::AdbClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRecordConfig {
    pub width: u32,
    pub height: u32,
    pub bit_rate: u32,
    pub display_id: Option<u32>,
}

impl ScreenRecordConfig {
    pub fn for_max_dimension(
        width: u32,
        height: u32,
        max_dimension: Option<u32>,
        bit_rate: u32,
    ) -> Self {
        let (width, height) = scaled_even_size(width, height, max_dimension);
        Self {
            width,
            height,
            bit_rate,
            display_id: None,
        }
    }
}

pub fn spawn_screenrecord(adb: &AdbClient, config: ScreenRecordConfig) -> Result<Child> {
    let mut command = Command::new(&adb.adb_path);
    if let Some(serial) = &adb.serial {
        command.arg("-s").arg(serial);
    }
    command.args([
        "exec-out",
        "screenrecord",
        "--output-format=h264",
        "--time-limit",
        "0",
        "--size",
        &format!("{}x{}", config.width, config.height),
        "--bit-rate",
        &config.bit_rate.to_string(),
    ]);
    if let Some(display_id) = config.display_id {
        command.args(["--display-id", &display_id.to_string()]);
    }
    command
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start adb screenrecord")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H264AccessUnit {
    pub data: Bytes,
    pub keyframe: bool,
}

#[derive(Default)]
pub struct AnnexBAccessUnitParser {
    stream: Vec<u8>,
    assembler: AccessUnitAssembler,
}

impl AnnexBAccessUnitParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<H264AccessUnit> {
        self.stream.extend_from_slice(chunk);
        let starts = start_codes(&self.stream);
        if starts.is_empty() {
            if self.stream.len() > 4 {
                let keep = self.stream.split_off(self.stream.len() - 4);
                self.stream = keep;
            }
            return Vec::new();
        }
        if starts[0].0 > 0 {
            self.stream.drain(..starts[0].0);
        }
        let starts = start_codes(&self.stream);
        if starts.len() < 2 {
            return Vec::new();
        }

        let mut out = Vec::new();
        for pair in starts.windows(2) {
            let (start, code_len) = pair[0];
            let end = pair[1].0;
            if let Some(frame) = self
                .assembler
                .push_nalu(trim_trailing_zeroes(&self.stream[start + code_len..end]))
            {
                out.push(frame);
            }
        }
        self.stream.drain(..starts.last().unwrap().0);
        out
    }

    pub fn flush_idle(&mut self) -> Vec<H264AccessUnit> {
        let mut out = Vec::new();
        if let Some((start, code_len)) = start_codes(&self.stream).first().copied() {
            let nalu = trim_trailing_zeroes(&self.stream[start + code_len..]);
            if !nalu.is_empty()
                && let Some(frame) = self.assembler.push_nalu(nalu)
            {
                out.push(frame);
            }
        }
        self.stream.clear();
        if let Some(frame) = self.assembler.flush() {
            out.push(frame);
        }
        out
    }
}

#[derive(Default)]
struct AccessUnitAssembler {
    data: Vec<u8>,
    has_vcl: bool,
    keyframe: bool,
    has_sps: bool,
    has_pps: bool,
    latest_sps: Option<Vec<u8>>,
    latest_pps: Option<Vec<u8>>,
}

impl AccessUnitAssembler {
    fn push_nalu(&mut self, nalu: &[u8]) -> Option<H264AccessUnit> {
        let nalu_type = nalu.first().map(|byte| byte & 0x1f)?;
        let starts_new_picture =
            matches!(nalu_type, 1 | 5) && self.has_vcl && first_mb_in_slice(nalu) == Some(0);
        let starts_new_prefix = self.has_vcl && matches!(nalu_type, 6..=9);
        let completed = (starts_new_picture || starts_new_prefix)
            .then(|| self.take_frame())
            .flatten();

        match nalu_type {
            1 | 5 => {
                self.has_vcl = true;
                self.keyframe |= nalu_type == 5;
            }
            7 => {
                self.has_sps = true;
                self.latest_sps = Some(nalu.to_vec());
            }
            8 => {
                self.has_pps = true;
                self.latest_pps = Some(nalu.to_vec());
            }
            _ => {}
        }
        append_nalu(&mut self.data, nalu);
        completed
    }

    fn flush(&mut self) -> Option<H264AccessUnit> {
        self.take_frame()
    }

    fn take_frame(&mut self) -> Option<H264AccessUnit> {
        if !self.has_vcl {
            return None;
        }
        let mut data = Vec::new();
        if self.keyframe {
            if !self.has_sps
                && let Some(sps) = &self.latest_sps
            {
                append_nalu(&mut data, sps);
            }
            if !self.has_pps
                && let Some(pps) = &self.latest_pps
            {
                append_nalu(&mut data, pps);
            }
        }
        data.append(&mut self.data);
        let frame = H264AccessUnit {
            data: Bytes::from(data),
            keyframe: self.keyframe,
        };
        self.has_vcl = false;
        self.keyframe = false;
        self.has_sps = false;
        self.has_pps = false;
        Some(frame)
    }
}

fn append_nalu(out: &mut Vec<u8>, nalu: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nalu);
}

fn start_codes(data: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= data.len() {
        if index + 4 <= data.len() && data[index..index + 4] == [0, 0, 0, 1] {
            starts.push((index, 4));
            index += 4;
        } else if data[index..index + 3] == [0, 0, 1] {
            starts.push((index, 3));
            index += 3;
        } else {
            index += 1;
        }
    }
    starts
}

fn trim_trailing_zeroes(mut data: &[u8]) -> &[u8] {
    while data.last() == Some(&0) {
        data = &data[..data.len() - 1];
    }
    data
}

fn first_mb_in_slice(nalu: &[u8]) -> Option<u32> {
    if !matches!(nalu.first().map(|byte| byte & 0x1f), Some(1 | 5)) {
        return None;
    }
    let mut rbsp = Vec::with_capacity(nalu.len().saturating_sub(1));
    let mut zeroes = 0;
    for &byte in nalu.get(1..)? {
        if zeroes >= 2 && byte == 3 {
            zeroes = 0;
            continue;
        }
        rbsp.push(byte);
        if byte == 0 {
            zeroes += 1;
        } else {
            zeroes = 0;
        }
    }
    read_unsigned_exp_golomb(&rbsp)
}

fn read_unsigned_exp_golomb(data: &[u8]) -> Option<u32> {
    let mut bit = 0usize;
    let mut leading_zeroes = 0u32;
    while bit < data.len() * 8 && !read_bit(data, bit)? {
        leading_zeroes += 1;
        bit += 1;
        if leading_zeroes >= 32 {
            return None;
        }
    }
    bit += 1;
    let mut suffix = 0u32;
    for _ in 0..leading_zeroes {
        suffix = (suffix << 1) | u32::from(read_bit(data, bit)?);
        bit += 1;
    }
    Some((1u32 << leading_zeroes) - 1 + suffix)
}

fn read_bit(data: &[u8], bit: usize) -> Option<bool> {
    let byte = *data.get(bit / 8)?;
    Some(byte & (1 << (7 - bit % 8)) != 0)
}

fn scaled_even_size(width: u32, height: u32, max_dimension: Option<u32>) -> (u32, u32) {
    let Some(max_dimension) = max_dimension else {
        return (width & !1, height & !1);
    };
    let longest = width.max(height);
    if longest <= max_dimension {
        return (width & !1, height & !1);
    }
    let max_dimension = max_dimension.max(2) & !1;
    if width >= height {
        let scaled_height =
            ((height as u64 * max_dimension as u64 / width as u64) as u32).max(2) & !1;
        (max_dimension, scaled_height)
    } else {
        let scaled_width =
            ((width as u64 * max_dimension as u64 / height as u64) as u32).max(2) & !1;
        (scaled_width, max_dimension)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annex_b(nalus: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for nalu in nalus {
            append_nalu(&mut out, nalu);
        }
        out
    }

    fn types(data: &[u8]) -> Vec<u8> {
        let starts = start_codes(data);
        starts
            .iter()
            .filter_map(|(start, code_len)| data.get(start + code_len).map(|byte| byte & 0x1f))
            .collect()
    }

    #[test]
    fn parses_every_byte_boundary() {
        let stream = annex_b(&[
            &[0x67, 0x42, 0x00, 0x1f],
            &[0x68, 0xce],
            &[0x65, 0x80, 0xaa],
            &[0x41, 0x80, 0xbb],
        ]);
        let mut parser = AnnexBAccessUnitParser::default();
        let mut frames = Vec::new();
        for byte in stream {
            frames.extend(parser.push(&[byte]));
        }
        frames.extend(parser.flush_idle());
        assert_eq!(frames.len(), 2);
        assert!(frames[0].keyframe);
        assert_eq!(types(&frames[0].data), vec![7, 8, 5]);
        assert!(!frames[1].keyframe);
        assert_eq!(types(&frames[1].data), vec![1]);
    }

    #[test]
    fn keeps_multiple_slices_in_one_picture() {
        let stream = annex_b(&[
            &[0x41, 0x80, 0xaa],
            &[0x41, 0x40, 0xbb],
            &[0x41, 0x80, 0xcc],
        ]);
        let mut parser = AnnexBAccessUnitParser::default();
        let mut frames = parser.push(&stream);
        frames.extend(parser.flush_idle());
        assert_eq!(frames.len(), 2);
        assert_eq!(types(&frames[0].data), vec![1, 1]);
        assert_eq!(types(&frames[1].data), vec![1]);
    }

    #[test]
    fn prepends_cached_parameter_sets_to_later_keyframes() {
        let stream = annex_b(&[
            &[0x67, 0x42, 0x00, 0x1f],
            &[0x68, 0xce],
            &[0x65, 0x80, 0xaa],
            &[0x41, 0x80, 0xbb],
            &[0x65, 0x80, 0xcc],
        ]);
        let mut parser = AnnexBAccessUnitParser::default();
        let mut frames = parser.push(&stream);
        frames.extend(parser.flush_idle());
        assert_eq!(types(&frames[2].data), vec![7, 8, 5]);
    }

    #[test]
    fn scales_to_even_dimensions() {
        assert_eq!(scaled_even_size(1080, 2424, Some(1280)), (570, 1280));
        assert_eq!(scaled_even_size(1081, 2425, None), (1080, 2424));
    }
}
