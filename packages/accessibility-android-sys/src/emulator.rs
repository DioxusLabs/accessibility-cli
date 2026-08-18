use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Streaming};

pub mod raw;
pub mod screenrecord;

pub mod protocol {
    pub mod controller {
        tonic::include_proto!("android.emulation.control");
    }

    pub mod rtc {
        tonic::include_proto!("android.emulation.control.v2");
    }
}

use protocol::controller::emulator_controller_client::EmulatorControllerClient;
use protocol::controller::{EmulatorStatus, Image, ImageFormat, InputEvent, input_event};
use protocol::rtc::rtc_client::RtcClient;
use protocol::rtc::{
    Id, JsepMsg, ReceiveJsepMessageRequest, ReceiveJsepMessageResponse, RtcStreamRequest,
    SendJsepMessageRequest,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorDiscovery {
    pub path: PathBuf,
    pub pid: Option<u32>,
    pub grpc_port: u16,
    pub grpc_token: Option<String>,
    pub properties: BTreeMap<String, String>,
}

impl EmulatorDiscovery {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).with_context(|| {
            format!("failed to read emulator discovery file {}", path.display())
        })?;
        let properties = parse_properties(&contents);
        let grpc_port = properties
            .get("grpc.port")
            .ok_or_else(|| anyhow!("{} has no grpc.port", path.display()))?
            .parse::<u16>()
            .with_context(|| format!("{} has an invalid grpc.port", path.display()))?;
        let grpc_token = properties
            .get("grpc.token")
            .filter(|token| !token.is_empty())
            .cloned();
        let pid = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("pid_"))
            .and_then(|pid| pid.parse().ok());
        Ok(Self {
            path: path.to_path_buf(),
            pid,
            grpc_port,
            grpc_token,
            properties,
        })
    }

    pub fn matches(&self, selector: &str) -> bool {
        if self
            .properties
            .values()
            .any(|value| value.eq_ignore_ascii_case(selector))
        {
            return true;
        }
        if self.pid.is_some_and(|pid| selector == pid.to_string()) {
            return true;
        }
        self.properties.get("port.serial").is_some_and(|port| {
            selector == port || selector.eq_ignore_ascii_case(&format!("emulator-{port}"))
        })
    }

    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.grpc_port)
    }
}

#[derive(Debug, Clone)]
pub struct EmulatorCapabilities {
    pub discovery: EmulatorDiscovery,
    pub version: String,
    pub booted: bool,
    pub rtc_v2: bool,
}

#[derive(Clone)]
pub struct EmulatorGrpcClient {
    discovery: EmulatorDiscovery,
    controller: EmulatorControllerClient<Channel>,
    rtc: RtcClient<Channel>,
}

impl EmulatorGrpcClient {
    pub async fn connect(discovery: EmulatorDiscovery) -> Result<Self> {
        let channel = Endpoint::from_shared(discovery.endpoint())?
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .connect()
            .await
            .with_context(|| {
                format!(
                    "failed to connect to Android Emulator gRPC endpoint {}",
                    discovery.endpoint()
                )
            })?;
        Ok(Self {
            discovery,
            controller: EmulatorControllerClient::new(channel.clone()),
            rtc: RtcClient::new(channel),
        })
    }

    pub fn discovery(&self) -> &EmulatorDiscovery {
        &self.discovery
    }

    pub async fn status(&mut self) -> Result<EmulatorStatus> {
        let request = self.authorized(())?;
        self.controller
            .get_status(request)
            .await
            .map(|response| response.into_inner())
            .context("Android Emulator getStatus failed")
    }

    pub async fn stream_screenshots(&mut self, format: ImageFormat) -> Result<Streaming<Image>> {
        let request = self.authorized(format)?;
        self.controller
            .stream_screenshot(request)
            .await
            .map(|response| response.into_inner())
            .context("Android Emulator streamScreenshot failed")
    }

    pub async fn send_input(&mut self, event: InputEvent) -> Result<()> {
        match event
            .r#type
            .ok_or_else(|| anyhow!("Android input event has no type"))?
        {
            input_event::Type::KeyEvent(event) => {
                let request = self.authorized(event)?;
                self.controller
                    .send_key(request)
                    .await
                    .context("Android Emulator sendKey failed")?;
            }
            input_event::Type::TouchEvent(event) => {
                let request = self.authorized(event)?;
                self.controller
                    .send_touch(request)
                    .await
                    .context("Android Emulator sendTouch failed")?;
            }
            input_event::Type::MouseEvent(event) => {
                let request = self.authorized(event)?;
                self.controller
                    .send_mouse(request)
                    .await
                    .context("Android Emulator sendMouse failed")?;
            }
            input_event::Type::WheelEvent(_) => {
                bail!("Android Emulator wheel input requires a streaming RPC")
            }
        }
        Ok(())
    }

    pub async fn begin_rtc_stream(&mut self) -> Result<Id> {
        let request = self.authorized(RtcStreamRequest {})?;
        let response = self
            .rtc
            .request_rtc_stream(request)
            .await
            .context("Android Emulator RTC v2 RequestRtcStream failed")?
            .into_inner();
        let id = response
            .id
            .filter(|id| !id.guid.is_empty())
            .ok_or_else(|| anyhow!("Android Emulator RTC v2 returned no stream id"))?;
        Ok(id)
    }

    pub async fn receive_jsep_stream(
        &mut self,
        id: Id,
    ) -> Result<Streaming<ReceiveJsepMessageResponse>> {
        let request = self.authorized(ReceiveJsepMessageRequest { id: Some(id) })?;
        self.rtc
            .receive_jsep_message_stream(request)
            .await
            .map(|response| response.into_inner())
            .context("Android Emulator RTC v2 ReceiveJsepMessageStream failed")
    }

    pub async fn send_jsep(&mut self, id: Id, message: impl Into<String>) -> Result<()> {
        let request = self.authorized(SendJsepMessageRequest {
            jsep_msg: Some(JsepMsg {
                id: Some(id),
                message: message.into(),
            }),
        })?;
        self.rtc
            .send_jsep_message(request)
            .await
            .context("Android Emulator RTC v2 SendJsepMessage failed")?;
        Ok(())
    }

    pub async fn end_rtc_stream(&mut self, id: Id) -> Result<()> {
        self.send_jsep(id, r#"{"bye":true}"#).await
    }

    pub async fn probe_capabilities(&mut self) -> Result<EmulatorCapabilities> {
        let status = self.status().await?;
        let id = self.begin_rtc_stream().await?;
        self.end_rtc_stream(id).await?;
        Ok(EmulatorCapabilities {
            discovery: self.discovery.clone(),
            version: status.version,
            booted: status.booted,
            rtc_v2: true,
        })
    }

    fn authorized<T>(&self, value: T) -> Result<Request<T>> {
        let mut request = Request::new(value);
        if let Some(token) = &self.discovery.grpc_token {
            let value = MetadataValue::try_from(format!("Bearer {token}"))
                .context("emulator gRPC token is not valid HTTP metadata")?;
            request.metadata_mut().insert("authorization", value);
        }
        Ok(request)
    }
}

pub fn discover_emulator(selector: Option<&str>) -> Result<EmulatorDiscovery> {
    discover_emulator_in(&discovery_directories(), selector)
}

pub fn discover_emulator_in(
    directories: &[PathBuf],
    selector: Option<&str>,
) -> Result<EmulatorDiscovery> {
    let mut paths = BTreeSet::new();
    for directory in directories {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to read emulator discovery directory {}",
                        directory.display()
                    )
                });
            }
        };
        for entry in entries {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with("pid_") && name.ends_with(".ini") {
                paths.insert(path);
            }
        }
    }

    let mut discoveries = paths
        .into_iter()
        .filter_map(|path| EmulatorDiscovery::from_file(path).ok())
        .collect::<Vec<_>>();
    if let Some(selector) = selector {
        discoveries.retain(|discovery| discovery.matches(selector));
    }

    match discoveries.len() {
        0 => match selector {
            Some(selector) => bail!("no running Android Emulator matches '{selector}'"),
            None => bail!("no running Android Emulator with a gRPC discovery file was found"),
        },
        1 => Ok(discoveries.remove(0)),
        count => match selector {
            Some(selector) => bail!("{count} running Android Emulators match '{selector}'"),
            None => bail!(
                "{count} Android Emulators are running; specify an AVD name, PID, serial, or gRPC port"
            ),
        },
    }
}

pub fn discovery_directories() -> Vec<PathBuf> {
    let mut directories = BTreeSet::new();
    directories.insert(std::env::temp_dir().join("avd/running"));
    if let Some(path) = std::env::var_os("ANDROID_EMULATOR_DISCOVERY") {
        directories.insert(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("ANDROID_AVD_HOME") {
        directories.insert(PathBuf::from(path).join("running"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        directories.insert(home.join(".android/avd/running"));
        if cfg!(target_os = "macos") {
            directories.insert(home.join("Library/Android/avd/running"));
            directories.insert(home.join("Library/Caches/TemporaryItems/avd/running"));
        }
    }
    directories.into_iter().collect()
}

fn parse_properties(contents: &str) -> BTreeMap<String, String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("accessibility-android-{name}-{nonce}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn parses_discovery_file() {
        let directory = test_directory("parse");
        let path = directory.join("pid_1234.ini");
        std::fs::write(
            &path,
            "grpc.port = 8554\ngrpc.token = secret\navd.name = Pixel_8\nport.serial = 5554\n",
        )
        .unwrap();
        let discovery = EmulatorDiscovery::from_file(&path).unwrap();
        assert_eq!(discovery.pid, Some(1234));
        assert_eq!(discovery.grpc_port, 8554);
        assert_eq!(discovery.grpc_token.as_deref(), Some("secret"));
        assert!(discovery.matches("Pixel_8"));
        assert!(discovery.matches("emulator-5554"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn selects_one_emulator() {
        let directory = test_directory("select");
        std::fs::write(
            directory.join("pid_1.ini"),
            "grpc.port=8554\navd.name=phone\n",
        )
        .unwrap();
        std::fs::write(
            directory.join("pid_2.ini"),
            "grpc.port=8555\navd.name=tablet\n",
        )
        .unwrap();
        let discovery =
            discover_emulator_in(std::slice::from_ref(&directory), Some("tablet")).unwrap();
        assert_eq!(discovery.grpc_port, 8555);
        assert!(discover_emulator_in(std::slice::from_ref(&directory), None).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
