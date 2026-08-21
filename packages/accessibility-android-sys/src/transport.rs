//! A narrow implementation of the ADB server smartsocket protocol.
//!
//! The workspace deliberately hand-rolls this small protocol slice instead of
//! adding `droidrun-adb` or `adbutils-rs`: the client needs only a stable set of
//! host, shell-v2, and exec services without expanding the dependency surface.

use std::net::SocketAddr;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;

#[derive(Clone)]
pub(crate) struct AdbTransport {
    server_addr: SocketAddr,
    adb_path: String,
}

impl AdbTransport {
    pub(crate) fn new(server_addr: SocketAddr, adb_path: &str) -> Self {
        Self {
            server_addr,
            adb_path: adb_path.to_string(),
        }
    }

    pub(crate) async fn host_query(&self, service: &str) -> Result<Vec<u8>> {
        let mut stream = self.connect().await?;
        write_service(&mut stream, service).await?;
        read_status(&mut stream).await?;
        read_length_prefixed(&mut stream, MAX_OUTPUT_LENGTH, "host response").await
    }

    pub(crate) async fn wait_for_device(&self, serial: Option<&str>) -> Result<()> {
        let service = match serial {
            Some(serial) => format!("host-serial:{serial}:wait-for-any-device"),
            None => "host:wait-for-any-device".to_string(),
        };
        let mut stream = self.connect().await?;
        write_service(&mut stream, &service).await?;
        read_status(&mut stream).await
    }

    pub(crate) async fn shell(&self, serial: Option<&str>, args: &[&str]) -> Result<ShellOutput> {
        let service = format!("shell,v2,raw:{}", args.join(" "));
        let mut stream = self.switch_to_device(serial).await?;
        write_service(&mut stream, &service).await?;
        read_status(&mut stream)
            .await
            .context("shell-v2-capable device/adb is required")?;
        read_shell_output(&mut stream).await
    }

    pub(crate) async fn exec(&self, serial: Option<&str>, args: &[&str]) -> Result<Vec<u8>> {
        let service = format!("exec:{}", args.join(" "));
        let mut stream = self.switch_to_device(serial).await?;
        write_service(&mut stream, &service).await?;
        read_status(&mut stream).await?;
        read_to_end_limited(&mut stream, MAX_OUTPUT_LENGTH, "exec output").await
    }

    async fn connect(&self) -> Result<TcpStream> {
        match TcpStream::connect(self.server_addr).await {
            Ok(stream) => Ok(stream),
            Err(error) if is_bootstrap_error(&error) => {
                self.start_server().await?;
                TcpStream::connect(self.server_addr).await.with_context(|| {
                    format!("failed to connect to ADB server at {}", self.server_addr)
                })
            }
            Err(error) => Err(error).with_context(|| {
                format!("failed to connect to ADB server at {}", self.server_addr)
            }),
        }
    }

    async fn start_server(&self) -> Result<()> {
        let child = Command::new(&self.adb_path)
            .arg("start-server")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "no ADB server at {} and ADB binary not found at '{}'. Install Android SDK Platform Tools.",
                    self.server_addr, self.adb_path
                )
            })?;
        let output = child
            .wait_with_output()
            .await
            .context("failed to wait for adb start-server")?;
        if !output.status.success() {
            bail!(
                "adb start-server failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    async fn switch_to_device(&self, serial: Option<&str>) -> Result<TcpStream> {
        let service = match serial {
            Some(serial) => format!("host:tport:serial:{serial}"),
            None => "host:tport:any".to_string(),
        };
        let mut stream = self.connect().await?;
        write_service(&mut stream, &service).await?;
        read_status(&mut stream).await?;
        let mut transport_id = [0; 8];
        stream
            .read_exact(&mut transport_id)
            .await
            .context("truncated ADB transport id")?;
        Ok(stream)
    }
}

#[derive(Debug, Default)]
pub(crate) struct ShellOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: u8,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellPacketId {
    Stdin = 0,
    Stdout = 1,
    Stderr = 2,
    Exit = 3,
    CloseStdin = 4,
    WindowSizeChange = 5,
    Invalid = 255,
}

impl ShellPacketId {
    fn decode(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Stdin),
            1 => Ok(Self::Stdout),
            2 => Ok(Self::Stderr),
            3 => Ok(Self::Exit),
            4 => Ok(Self::CloseStdin),
            5 => Ok(Self::WindowSizeChange),
            255 => Ok(Self::Invalid),
            value => bail!("unknown shell-v2 packet id {value}"),
        }
    }
}

async fn write_service(stream: &mut TcpStream, service: &str) -> Result<()> {
    let length = service.len();
    if length > MAX_SERVICE_LENGTH {
        bail!("ADB service string exceeds {MAX_SERVICE_LENGTH} bytes");
    }
    let header = format!("{length:04x}");
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(service.as_bytes()).await?;
    Ok(())
}

async fn read_status(stream: &mut TcpStream) -> Result<()> {
    let mut status = [0; 4];
    stream
        .read_exact(&mut status)
        .await
        .context("truncated ADB response status")?;
    match &status {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let message = read_length_prefixed(stream, MAX_PACKET_LENGTH, "ADB failure").await?;
            bail!("ADB server failure: {}", String::from_utf8_lossy(&message));
        }
        _ => bail!("malformed ADB response status"),
    }
}

async fn read_length_prefixed<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_length: usize,
    kind: &str,
) -> Result<Vec<u8>> {
    let length = read_hex_length(reader, kind).await?;
    if length > max_length {
        bail!("{kind} length {length} exceeds maximum {max_length}");
    }
    let mut payload = vec![0; length];
    reader
        .read_exact(&mut payload)
        .await
        .with_context(|| format!("truncated {kind} payload"))?;
    Ok(payload)
}

async fn read_shell_output(stream: &mut TcpStream) -> Result<ShellOutput> {
    let mut output = ShellOutput::default();
    loop {
        let mut header = [0; 5];
        stream
            .read_exact(&mut header)
            .await
            .context("truncated shell-v2 packet header")?;
        let packet_id = ShellPacketId::decode(header[0])?;
        let length = u32::from_le_bytes(header[1..].try_into().unwrap()) as usize;
        if length > MAX_PACKET_LENGTH {
            bail!("shell-v2 packet length {length} exceeds maximum {MAX_PACKET_LENGTH}");
        }
        match packet_id {
            ShellPacketId::Stdout => {
                if length
                    > MAX_OUTPUT_LENGTH.saturating_sub(output.stdout.len() + output.stderr.len())
                {
                    bail!("shell-v2 output exceeds maximum {MAX_OUTPUT_LENGTH} bytes");
                }
                let mut payload = vec![0; length];
                stream.read_exact(&mut payload).await?;
                output.stdout.extend(payload);
            }
            ShellPacketId::Stderr => {
                if length
                    > MAX_OUTPUT_LENGTH.saturating_sub(output.stdout.len() + output.stderr.len())
                {
                    bail!("shell-v2 output exceeds maximum {MAX_OUTPUT_LENGTH} bytes");
                }
                let mut payload = vec![0; length];
                stream.read_exact(&mut payload).await?;
                output.stderr.extend(payload);
            }
            ShellPacketId::Exit => {
                if length != 1 {
                    bail!("shell-v2 exit packet must contain one byte");
                }
                let mut exit_code = [0];
                stream.read_exact(&mut exit_code).await?;
                output.exit_code = exit_code[0];
                let mut trailing = [0];
                if stream.read(&mut trailing).await? != 0 {
                    bail!("shell-v2 stream has data after the exit packet");
                }
                return Ok(output);
            }
            ShellPacketId::Stdin
            | ShellPacketId::CloseStdin
            | ShellPacketId::WindowSizeChange
            | ShellPacketId::Invalid => bail!("unexpected shell-v2 packet id {}", packet_id as u8),
        }
    }
}

async fn read_to_end_limited<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_length: usize,
    kind: &str,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0; IO_BUFFER_LENGTH];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(output);
        }
        if read > max_length.saturating_sub(output.len()) {
            bail!("{kind} exceeds maximum {max_length} bytes");
        }
        output.extend_from_slice(&buffer[..read]);
    }
}

async fn read_hex_length<R: AsyncRead + Unpin>(reader: &mut R, kind: &str) -> Result<usize> {
    let mut header = [0; 4];
    reader
        .read_exact(&mut header)
        .await
        .with_context(|| format!("truncated {kind} length"))?;
    let text = std::str::from_utf8(&header).with_context(|| format!("invalid {kind} length"))?;
    usize::from_str_radix(text, 16).with_context(|| format!("invalid {kind} length"))
}

fn is_bootstrap_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::ConnectionRefused
}

/// The default local ADB server endpoint.
pub(crate) const DEFAULT_SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 5037);

const MAX_SERVICE_LENGTH: usize = 1024;

/// The maximum payload accepted in one shell-v2 packet.
const MAX_PACKET_LENGTH: usize = 1024 * 1024;

/// The maximum output accumulated from one shell or exec request.
const MAX_OUTPUT_LENGTH: usize = 64 * 1024 * 1024;

const IO_BUFFER_LENGTH: usize = 8192;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AdbClient;
    use std::time::Duration;
    use tokio::net::{TcpListener, TcpStream};

    async fn bind_listener() -> (TcpListener, SocketAddr) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        (listener, address)
    }

    async fn read_service(stream: &mut TcpStream) -> String {
        let mut header = [0; 4];
        stream.read_exact(&mut header).await.unwrap();
        let length = usize::from_str_radix(std::str::from_utf8(&header).unwrap(), 16).unwrap();
        let mut service = vec![0; length];
        stream.read_exact(&mut service).await.unwrap();
        String::from_utf8(service).unwrap()
    }

    async fn write_host_response(stream: &mut TcpStream, payload: &[u8]) {
        stream.write_all(b"OKAY").await.unwrap();
        stream
            .write_all(format!("{:04x}", payload.len()).as_bytes())
            .await
            .unwrap();
        stream.write_all(payload).await.unwrap();
    }

    async fn write_shell_packet(stream: &mut TcpStream, id: u8, payload: &[u8]) {
        stream.write_all(&[id]).await.unwrap();
        stream
            .write_all(&(payload.len() as u32).to_le_bytes())
            .await
            .unwrap();
        stream.write_all(payload).await.unwrap();
    }

    async fn switch_to_shell(
        stream: &mut TcpStream,
        expected_switch: &str,
        expected_command: &str,
    ) {
        assert_eq!(read_service(stream).await, expected_switch);
        stream.write_all(b"OKAYtid-1234").await.unwrap();
        assert_eq!(
            read_service(stream).await,
            format!("shell,v2,raw:{expected_command}")
        );
        stream.write_all(b"OKAY").await.unwrap();
    }

    // Miri's default isolation does not support socket syscalls; these tests
    // still run under the normal test harness.
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn host_devices_filters_non_devices() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(read_service(&mut stream).await, "host:devices");
            write_host_response(
                &mut stream,
                b"phone\tdevice\noffline\toffline\nunauthorized\tunauthorized\n",
            )
            .await;
        });
        let adb = AdbClient::new(Some("ignored")).with_server_addr(address);
        assert_eq!(adb.connected_devices().await.unwrap(), ["phone"]);
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn host_devices_parses_single_device_payload() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(read_service(&mut stream).await, "host:devices");
            write_host_response(&mut stream, b"emulator-5554\tdevice\n").await;
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        assert_eq!(adb.connected_devices().await.unwrap(), ["emulator-5554"]);
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn transport_switch_reads_tid_and_shell_request() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            switch_to_shell(&mut stream, "host:tport:serial:emulator-5554", "echo ok").await;
            write_shell_packet(&mut stream, ShellPacketId::Exit as u8, &[0]).await;
        });
        let adb = AdbClient::new(Some("emulator-5554")).with_server_addr(address);
        assert_eq!(adb.shell(&["echo", "ok"]).await.unwrap(), "");
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn shell_collects_split_stdout_stderr_and_exit() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            switch_to_shell(&mut stream, "host:tport:any", "echo ok").await;
            write_shell_packet(&mut stream, 1, b"hello ").await;
            write_shell_packet(&mut stream, 2, b"warning").await;
            write_shell_packet(&mut stream, 1, b"world").await;
            write_shell_packet(&mut stream, 3, &[0]).await;
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        let output = adb.shell_raw(&["echo", "ok"]).await.unwrap();
        assert_eq!(output, b"hello world");
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn nonzero_shell_exit_includes_stderr() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            switch_to_shell(&mut stream, "host:tport:any", "false").await;
            write_shell_packet(&mut stream, 2, b"bad command").await;
            write_shell_packet(&mut stream, 3, &[7]).await;
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        let error = adb.shell(&["false"]).await.unwrap_err();
        assert!(error.to_string().contains("exit code 7"));
        assert!(error.to_string().contains("bad command"));
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fail_response_surfaces_server_message() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(read_service(&mut stream).await, "host:version");
            stream.write_all(b"FAIL000eserver says no").await.unwrap();
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        let error = adb.server_version().await.unwrap_err();
        assert!(error.to_string().contains("server says no"));
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn truncated_status_and_payload_error() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(read_service(&mut stream).await, "host:version");
            stream.write_all(b"OK").await.unwrap();
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        assert!(adb.server_version().await.is_err());
        server.await.unwrap();

        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(read_service(&mut stream).await, "host:devices");
            stream.write_all(b"OKAY0004ab").await.unwrap();
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        assert!(adb.connected_devices().await.is_err());
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn shell_eof_before_exit_is_an_error() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            switch_to_shell(&mut stream, "host:tport:any", "echo").await;
            write_shell_packet(&mut stream, 1, b"partial output").await;
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        assert!(adb.shell_raw(&["echo"]).await.is_err());
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn oversized_and_unknown_shell_packets_are_rejected() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            switch_to_shell(&mut stream, "host:tport:any", "echo").await;
            stream.write_all(&[1]).await.unwrap();
            stream
                .write_all(&((MAX_PACKET_LENGTH as u32) + 1).to_le_bytes())
                .await
                .unwrap();
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        assert!(adb.shell_raw(&["echo"]).await.is_err());
        server.await.unwrap();

        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            switch_to_shell(&mut stream, "host:tport:any", "echo").await;
            stream.write_all(&[99, 0, 0, 0, 0]).await.unwrap();
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        assert!(adb.shell_raw(&["echo"]).await.is_err());
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn exec_preserves_binary_output() {
        let (listener, address) = bind_listener().await;
        let expected = b"\0png\r\n\xff";
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(read_service(&mut stream).await, "host:tport:any");
            stream.write_all(b"OKAYtid-1234").await.unwrap();
            assert_eq!(read_service(&mut stream).await, "exec:screencap -p");
            stream.write_all(b"OKAY").await.unwrap();
            stream.write_all(expected).await.unwrap();
        });
        let adb = AdbClient::new(None).with_server_addr(address);
        assert_eq!(adb.exec_out(&["screencap", "-p"]).await.unwrap(), expected);
        server.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn refused_server_reports_missing_server_and_binary() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let adb = AdbClient::with_adb_path(None, "/no/such/adb").with_server_addr(address);
        let error = adb.server_version().await.unwrap_err().to_string();
        assert!(error.contains("no ADB server"));
        assert!(error.contains("ADB binary not found"));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn request_timeout_drops_stalled_socket() {
        let (listener, address) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let adb = AdbClient::new(None)
            .with_server_addr(address)
            .with_timeout(Duration::from_millis(50));
        let error = adb.server_version().await.unwrap_err();
        assert!(error.to_string().contains("timed out"));
        server.abort();
    }
}
