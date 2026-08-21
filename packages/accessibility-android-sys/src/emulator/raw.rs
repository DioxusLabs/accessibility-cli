use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use memmap2::{MmapMut, MmapOptions};
use tonic::Streaming;

use super::protocol::controller::image_format::ImgFormat;
use super::protocol::controller::image_transport::TransportChannel;
use super::protocol::controller::{Image, ImageFormat, ImageTransport};
use super::{EmulatorDiscovery, EmulatorGrpcClient, discover_emulator};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawFrameTransport {
    Grpc,
    Mmap,
}

#[derive(Debug, Clone, Copy)]
pub struct RawFrameConfig {
    pub width: u32,
    pub height: u32,
    pub display: u32,
    pub transport: RawFrameTransport,
}

#[derive(Debug)]
pub struct RawFrame {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub sequence: u32,
    pub timestamp_us: u64,
    pub rotation: i32,
}

pub struct RawFrameStream {
    discovery: EmulatorDiscovery,
    stream: Streaming<Image>,
    mapping: Option<MmapMut>,
    mapping_file: Option<File>,
    mapping_path: Option<PathBuf>,
}

impl RawFrameStream {
    pub async fn start(selector: Option<&str>, config: RawFrameConfig) -> Result<Self> {
        if config.width == 0 || config.height == 0 {
            bail!("raw Android capture requires non-zero dimensions");
        }
        let discovery = discover_emulator(selector).await?;
        Self::start_with_discovery(discovery, config).await
    }

    pub async fn start_with_discovery(
        discovery: EmulatorDiscovery,
        config: RawFrameConfig,
    ) -> Result<Self> {
        let mut client = EmulatorGrpcClient::connect(discovery.clone()).await?;
        let capacity = rgba_len(config.width, config.height)?;
        let (mapping, mapping_file, mapping_path, transport) = match config.transport {
            RawFrameTransport::Grpc => (None, None, None, None),
            RawFrameTransport::Mmap => {
                let path = mapping_path();
                let file = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .read(true)
                    .write(true)
                    .open(&path)
                    .with_context(|| {
                        format!("creating emulator frame mapping {}", path.display())
                    })?;
                file.set_len(capacity as u64)?;
                let mapping = unsafe { MmapOptions::new().len(capacity).map_mut(&file)? };
                let transport = ImageTransport {
                    channel: TransportChannel::Mmap as i32,
                    handle: file_uri(&path),
                };
                (Some(mapping), Some(file), Some(path), Some(transport))
            }
        };
        let stream = client
            .stream_screenshots(ImageFormat {
                format: ImgFormat::Rgba8888 as i32,
                rotation: None,
                width: config.width,
                height: config.height,
                display: config.display,
                transport,
            })
            .await?;
        Ok(Self {
            discovery,
            stream,
            mapping,
            mapping_file,
            mapping_path,
        })
    }

    pub fn discovery(&self) -> &EmulatorDiscovery {
        &self.discovery
    }

    pub async fn next_frame(&mut self) -> Result<Option<RawFrame>> {
        loop {
            let Some(image) = self.stream.message().await? else {
                return Ok(None);
            };
            let format = image.format.context("emulator frame has no format")?;
            let width = format.width.max(image.width);
            let height = format.height.max(image.height);
            if width == 0 || height == 0 {
                continue;
            }
            let pixels = top_down_rgba(self.mapping.as_deref(), image.image, width, height)?;
            return Ok(Some(RawFrame {
                pixels,
                width,
                height,
                sequence: image.seq,
                timestamp_us: image.timestamp_us,
                rotation: format.rotation.map_or(0, |rotation| rotation.rotation),
            }));
        }
    }
}

impl Drop for RawFrameStream {
    fn drop(&mut self) {
        self.mapping.take();
        self.mapping_file.take();
        if let Some(path) = self.mapping_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn rgba_len(width: u32, height: u32) -> Result<usize> {
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|bytes| bytes as usize)
        .context("Android frame dimensions overflow")
}

/// Normalizes a frame to top-down RGBA. MMAP frames arrive bottom-up and are
/// flipped; gRPC frames already arrive top-down and pass through unchanged.
fn top_down_rgba(
    mapping: Option<&[u8]>,
    image: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let expected = rgba_len(width, height)?;
    if let Some(mapping) = mapping {
        let source = mapping
            .get(..expected)
            .ok_or_else(|| anyhow!("emulator frame exceeds its memory mapping"))?;
        Ok(copy_bottom_up_rgba(source, width, height))
    } else {
        if image.len() != expected {
            bail!(
                "RGBA frame is {} bytes, expected {expected} for {width}x{height}",
                image.len()
            );
        }
        Ok(image)
    }
}

fn copy_bottom_up_rgba(source: &[u8], width: u32, height: u32) -> Vec<u8> {
    let stride = width as usize * 4;
    let mut pixels = vec![0; source.len()];
    for row in 0..height as usize {
        let source_start = (height as usize - 1 - row) * stride;
        let target_start = row * stride;
        pixels[target_start..target_start + stride]
            .copy_from_slice(&source[source_start..source_start + stride]);
    }
    pixels
}

fn mapping_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "accessibility-emulator-raw-{}-{nonce}.rgba",
        std::process::id()
    ))
}

fn file_uri(path: &std::path::Path) -> String {
    #[cfg(target_os = "windows")]
    {
        format!("file:///{}", path.display().to_string().replace('\\', "/"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("file://{}", path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_bottom_up_rgba() {
        assert_eq!(
            copy_bottom_up_rgba(&[1, 2, 3, 4, 5, 6, 7, 8], 1, 2),
            vec![5, 6, 7, 8, 1, 2, 3, 4]
        );
    }

    /// A 2x3 frame with a distinct value per row; any vertical inversion
    /// reorders the rows and fails the comparison.
    fn rows_2x3(top_to_bottom: [u8; 3]) -> Vec<u8> {
        top_to_bottom
            .iter()
            .flat_map(|&row| std::iter::repeat_n(row, 8))
            .collect()
    }

    #[test]
    fn grpc_frames_pass_through_top_down() {
        let top_down = rows_2x3([1, 2, 3]);
        assert_eq!(
            top_down_rgba(None, top_down.clone(), 2, 3).unwrap(),
            top_down
        );
    }

    #[test]
    fn mmap_frames_are_flipped_to_top_down() {
        let bottom_up = rows_2x3([3, 2, 1]);
        assert_eq!(
            top_down_rgba(Some(&bottom_up), Vec::new(), 2, 3).unwrap(),
            rows_2x3([1, 2, 3])
        );
    }
}
