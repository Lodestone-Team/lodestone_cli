//! A tiny mock Lodestone core for the integration tests: plain or TLS HTTP/1.1 on
//! 127.0.0.1, one thread per connection, and WebSockets through tungstenite. Handlers get
//! each request and return a reply; every request is recorded for assertions.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// The path without the query, still percent-encoded.
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// A query parameter, percent-decoded.
    pub fn query_param(&self, name: &str) -> Option<String> {
        self.query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == name).then(|| percent_decode(v))
        })
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("JSON body")
    }

    pub fn is(&self, method: &str, path: &str) -> bool {
        self.method == method && self.path == path
    }
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

pub enum Reply {
    Json(u16, Value),
    Text(u16, String),
    Raw {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Accept a WebSocket and send these messages, then close.
    WebSocket(Vec<Value>),
}

impl Reply {
    pub fn ok(value: Value) -> Self {
        Reply::Json(200, value)
    }

    pub fn error(status: u16, kind: &str, message: &str) -> Self {
        Reply::Json(
            status,
            serde_json::json!({"kind": kind, "causes": [message]}),
        )
    }
}

type Handler = dyn Fn(&Request) -> Reply + Send + Sync;

pub struct MockServer {
    pub port: u16,
    pub tls: bool,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl MockServer {
    pub fn start(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        Self::start_inner(None, handler)
    }

    /// A TLS server with a certificate and key from `tests/fixtures`.
    pub fn start_tls(
        cert: &str,
        key: &str,
        handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    ) -> Self {
        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        let certs = CertificateDer::pem_file_iter(fixture(cert))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_file(fixture(key)).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
        Self::start_inner(Some(Arc::new(config)), handler)
    }

    fn start_inner(
        tls: Option<Arc<rustls::ServerConfig>>,
        handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let recorded = requests.clone();
        let is_tls = tls.is_some();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let handler = handler.clone();
                let recorded = recorded.clone();
                let tls = tls.clone();
                std::thread::spawn(move || {
                    let _ = match tls {
                        Some(config) => {
                            let conn = rustls::ServerConnection::new(config).unwrap();
                            let stream = rustls::StreamOwned::new(conn, stream);
                            serve(stream, &*handler, &recorded)
                        }
                        None => serve(stream, &*handler, &recorded),
                    };
                });
            }
        });
        Self {
            port,
            tls: is_tls,
            requests,
        }
    }

    pub fn url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://127.0.0.1:{}", self.port)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// Requests other than the instance list (which name resolution sends).
    pub fn requests_to(&self, method: &str, path: &str) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|r| r.is(method, path))
            .collect()
    }
}

/// A stream the server can read a request head from and still hand to tungstenite.
struct Replay<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S: Read> Read for Replay<S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.prefix[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.inner.read(buf)
    }
}

impl<S: Write> Write for Replay<S> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn serve<S: Read + Write>(
    mut stream: S,
    handler: &Handler,
    recorded: &Mutex<Vec<Request>>,
) -> std::io::Result<()> {
    loop {
        // read the head byte by byte, so nothing after it is consumed
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte)? == 0 {
                return Ok(());
            }
            head.push(byte[0]);
        }
        let text = String::from_utf8_lossy(&head).into_owned();
        let mut lines = BufReader::new(text.as_bytes()).lines();
        let request_line = lines.next().unwrap()?;
        let mut parts = request_line.split(' ');
        let method = parts.next().unwrap_or_default().to_string();
        let target = parts.next().unwrap_or_default().to_string();
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (target.clone(), String::new()),
        };
        let mut headers = Vec::new();
        for line in lines {
            let line = line?;
            if let Some((k, v)) = line.split_once(':') {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        let header = |name: &str| {
            headers
                .iter()
                .find(|(k, _): &&(String, String)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        };
        let is_upgrade = header("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
        let mut body = Vec::new();
        if let Some(len) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
            body.resize(len, 0);
            stream.read_exact(&mut body)?;
        } else if header("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
            body = read_chunked(&mut stream)?;
        }
        let request = Request {
            method,
            path,
            query,
            headers,
            body,
        };
        recorded.lock().unwrap().push(request.clone());
        let reply = handler(&request);
        if is_upgrade {
            let messages = match reply {
                Reply::WebSocket(messages) => messages,
                other => return write_reply(&mut stream, other),
            };
            let replay = Replay {
                prefix: head,
                pos: 0,
                inner: stream,
            };
            let mut socket = tokio_tungstenite::tungstenite::accept(replay)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            for message in messages {
                socket
                    .send(tokio_tungstenite::tungstenite::Message::text(
                        message.to_string(),
                    ))
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
            }
            let _ = socket.close(None);
            // let the client read the close frame
            for _ in 0..10 {
                if socket.read().is_err() {
                    break;
                }
            }
            return Ok(());
        }
        write_reply(&mut stream, reply)?;
    }
}

fn read_chunked<S: Read>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while !line.ends_with(b"\r\n") {
            stream.read_exact(&mut byte)?;
            line.push(byte[0]);
        }
        let size = usize::from_str_radix(String::from_utf8_lossy(&line).trim(), 16).unwrap();
        let mut chunk = vec![0u8; size + 2];
        stream.read_exact(&mut chunk)?;
        if size == 0 {
            return Ok(body);
        }
        body.extend_from_slice(&chunk[..size]);
    }
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        _ => "Status",
    }
}

fn write_reply<S: Write>(stream: &mut S, reply: Reply) -> std::io::Result<()> {
    let (status, mut headers, body) = match reply {
        Reply::Json(status, value) => (
            status,
            vec![("Content-Type".to_string(), "application/json".to_string())],
            value.to_string().into_bytes(),
        ),
        Reply::Text(status, text) => (
            status,
            vec![("Content-Type".to_string(), "text/plain".to_string())],
            text.into_bytes(),
        ),
        Reply::Raw {
            status,
            headers,
            body,
        } => (status, headers, body),
        Reply::WebSocket(_) => (400, vec![], b"expected a WebSocket".to_vec()),
    };
    headers.push(("Content-Length".to_string(), body.len().to_string()));
    let mut out = format!("HTTP/1.1 {status} {}\r\n", status_text(status));
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    stream.write_all(out.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()
}

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// The URL-safe, unpadded base64 the file routes use.
pub fn b64(path: &str) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = path.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..=chunk.len() {
            out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

const PROXY_VARIABLES: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];
const LOOPBACK: &str = "127.0.0.1,localhost,::1";

/// Runs the CLI with a private config folder.
pub struct Cli {
    pub config_dir: tempfile::TempDir,
    pub env: Vec<(String, String)>,
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}): {}", self.stdout))
    }

    pub fn error_json(&self) -> Value {
        serde_json::from_str(self.stderr.trim())
            .unwrap_or_else(|e| panic!("stderr is not JSON ({e}): {}", self.stderr))
    }
}

impl Cli {
    pub fn new() -> Self {
        Self {
            config_dir: tempfile::tempdir().unwrap(),
            env: Vec::new(),
        }
    }

    /// Talks to `server` with a token from the environment.
    pub fn with_env(server: &MockServer) -> Self {
        let mut cli = Self::new();
        cli.env.push(("LODESTONE_URL".into(), server.url()));
        cli.env
            .push(("LODESTONE_TOKEN".into(), "test-token".into()));
        cli
    }

    pub fn run(&self, args: &[&str]) -> Run {
        self.run_with_stdin(args, "")
    }

    pub fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Run {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lodestone_cli"));
        command
            .args(args)
            .env("LODESTONE_CLI_CONFIG_DIR", self.config_dir.path())
            .env_remove("LODESTONE_URL")
            .env_remove("LODESTONE_TOKEN")
            .env_remove("LODESTONE_USERNAME")
            .env_remove("LODESTONE_PASSWORD")
            .env_remove("LODESTONE_FINGERPRINT")
            .env_remove("LODESTONE_CA_CERT")
            // nothing the CLI writes by accident lands in the repository
            .current_dir(self.config_dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The mock core is on loopback: a proxy from the developer's or CI's environment
        // (or the system settings, which reqwest reads too) must not get in between.
        for proxy in PROXY_VARIABLES {
            command.env_remove(proxy);
        }
        command.env("NO_PROXY", LOOPBACK).env("no_proxy", LOOPBACK);
        for (k, v) in &self.env {
            command.env(k, v);
        }
        let mut child = command.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let _ = input.write_all(stdin.as_bytes());
        drop(input);
        let (code, stdout, stderr) = wait_with_timeout(child);
        Run {
            code,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        }
    }

    pub fn profiles_path(&self) -> PathBuf {
        self.config_dir.path().join("profiles.json")
    }
}

/// Waits for the CLI, killing it after 180 seconds (well above the tests' own timeouts,
/// which only bound a wait that normally ends at once). Returns the exit code and output.
fn wait_with_timeout(mut child: std::process::Child) -> (i32, Vec<u8>, Vec<u8>) {
    fn reader(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.read_to_end(&mut out);
            out
        })
    }
    let stdout = reader(child.stdout.take().unwrap());
    let stderr = reader(child.stderr.take().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the CLI did not finish within 180 seconds");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    (
        status.code().unwrap_or(-1),
        stdout.join().unwrap(),
        stderr.join().unwrap(),
    )
}

/// The instance list most tests use.
pub fn instance_list() -> Value {
    serde_json::json!([
        {
            "uuid": "INSTANCE_11111111-aaaa", "name": "Survival",
            "game_type": {"type": "MinecraftJava", "variant": {"type": "Fabric"}},
            "description": "", "version": "1.21.1", "port": 25565, "creation_time": 1700000000,
            "path": "/srv/survival", "auto_start": false, "restart_on_crash": false,
            "state": "Stopped", "player_count": null, "max_player_count": null, "player_list": null
        },
        {
            "uuid": "INSTANCE_22222222-bbbb", "name": "creative",
            "game_type": {"type": "MinecraftJava", "variant": {"type": "Paper"}},
            "description": "", "version": "1.20.4", "port": 25566, "creation_time": 1700000001,
            "path": "/srv/creative", "auto_start": false, "restart_on_crash": false,
            "state": "Running", "player_count": 1, "max_player_count": 20, "player_list": []
        }
    ])
}
