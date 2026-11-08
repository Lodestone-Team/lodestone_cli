//! The HTTP client for the core's REST API (`/api/v1`).
//!
//! Requests carry the login token in `Authorization: Bearer`. The token is never printed:
//! `--verbose` shows the method and path only.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, StatusCode, Url};
use serde_json::Value;

use super::error::{kind, CliError, CliResult};
use super::tls::{self, Trust};

/// How long a single read from the core may take before the request fails. Transfers have
/// no overall limit; a core that stops answering does.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// A request body.
pub enum Body {
    Empty,
    Json(Value),
    Bytes(Vec<u8>),
    /// A streamed body with its content type and, if known, its length.
    Stream {
        body: reqwest::Body,
        content_type: String,
        length: Option<u64>,
    },
}

/// A connection to one core: its URL, the token, and how its certificate is checked.
pub struct Connection {
    /// The core's base URL, such as `https://host:16662`.
    pub base: Url,
    token: Option<String>,
    pub trust: Trust,
    pub tls_config: Arc<rustls::ClientConfig>,
    client: reqwest::Client,
    verbose: bool,
}

/// Parses a core URL as typed by a user: `host`, `host:port`, `https://host:port`, ...
/// A missing scheme means `https`, a missing port means the core's default port 16662.
pub fn parse_core_url(input: &str) -> CliResult<Url> {
    let input = input.trim().trim_end_matches('/');
    if input.is_empty() {
        return Err(CliError::usage("The core URL is empty"));
    }
    if input.ends_with(':') {
        return Err(CliError::usage(format!("`{input}` has no host")));
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let mut url = Url::parse(&with_scheme)
        .map_err(|e| CliError::usage(format!("`{input}` is not a valid URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CliError::usage(format!(
            "`{input}`: the core URL must start with https:// or http://"
        )));
    }
    if url.host_str().is_none() {
        return Err(CliError::usage(format!("`{input}` has no host")));
    }
    if url.port().is_none() && !input_has_explicit_port(&with_scheme) {
        url.set_port(Some(16662))
            .map_err(|_| CliError::usage(format!("`{input}` cannot have a port")))?;
    }
    // `/api/v1` is added to every request; accept URLs copied with it
    let path = url.path().trim_end_matches('/').to_string();
    let path = path.strip_suffix("/api/v1").unwrap_or(&path).to_string();
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CliError::usage(
            "Do not put a user name or password in the core URL",
        ));
    }
    Ok(url)
}

/// `Url` drops a port equal to the scheme's default (`https://h:443`), so check the text.
fn input_has_explicit_port(url: &str) -> bool {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    match host_port.rfind(']') {
        Some(end) => host_port[end..].contains(':'),
        None => host_port.contains(':'),
    }
}

/// Whether the URL's host is this machine.
pub fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        None => false,
    }
}

/// The innermost cause of an error, such as "Connection refused (os error 61)".
fn root_cause(e: &reqwest::Error) -> String {
    let mut current: &dyn std::error::Error = e;
    while let Some(source) = current.source() {
        current = source;
    }
    current.to_string()
}

/// Describes a reqwest error with its causes, which hold the TLS or network reason.
fn describe(e: &reqwest::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<&dyn std::error::Error> = Some(e);
    while let Some(err) = current {
        let text = err.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        current = err.source();
    }
    parts.join(": ")
}

impl Connection {
    pub fn new(base: Url, token: Option<String>, trust: Trust, verbose: bool) -> CliResult<Self> {
        let tls_config = Arc::new(tls::client_config(&trust)?);
        let client = reqwest::Client::builder()
            .use_preconfigured_tls((*tls_config).clone())
            .connect_timeout(tls::CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .user_agent(concat!("lodestone_cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| CliError::new(kind::TLS, format!("HTTP client setup failed: {e}")))?;
        Ok(Self {
            base,
            token,
            trust,
            tls_config,
            client,
            verbose,
        })
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn set_token(&mut self, token: String) {
        self.token = Some(token);
    }

    /// The URL of an API route: the base URL, `/api/v1`, then `segments`, each
    /// percent-encoded as one path segment.
    pub fn api_url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .expect("http(s) URLs have path segments");
            path.pop_if_empty().extend(["api", "v1"]).extend(segments);
        }
        url
    }

    fn request(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> reqwest::RequestBuilder {
        let mut url = self.api_url(segments);
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }
        if self.verbose {
            // the path only: never the query (the event stream filter holds a token there)
            // or a header
            eprintln!("> {method} {}", crate::api::keys::redact(url.path()));
        }
        let mut builder = self.client.request(method, url);
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        builder
    }

    fn network_error(&self, e: &reqwest::Error) -> CliError {
        let rustls_error = tls::find_rustls_error(e);
        if let Some(error) = rustls_error.filter(|r| tls::is_certificate_error(r)) {
            let mut message = format!(
                "The certificate of {} is not accepted: {}",
                self.base,
                tls::describe_rustls_error(error)
            );
            if self.trust == Trust::System {
                message.push_str(
                    ". Log in again to check and pin it (`lodestone_cli login`), or pass --ca-cert",
                );
            }
            return CliError::new(kind::UNTRUSTED_CERTIFICATE, message);
        }
        if let Some(error) = rustls_error {
            return CliError::new(
                kind::TLS,
                format!(
                    "TLS with {} failed: {}",
                    self.base,
                    tls::describe_rustls_error(error)
                ),
            );
        }
        let mut message = if e.is_connect() {
            format!("Could not connect to {}: {}", self.base, root_cause(e))
        } else if e.is_timeout() {
            format!("The core at {} stopped answering", self.base)
        } else {
            format!("Request to {} failed: {}", self.base, describe(e))
        };
        if self.base.scheme() == "http" && e.is_connect() {
            message.push_str(". Cores serve HTTPS by default; try https://");
        }
        CliError::new(kind::NETWORK, message)
    }

    /// Sends a request and returns the response if its status is a success.
    pub async fn send(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Body,
    ) -> CliResult<reqwest::Response> {
        let builder = self.request(method, segments, query);
        let builder = match body {
            Body::Empty => builder,
            Body::Json(value) => builder.json(&value),
            Body::Bytes(bytes) => builder
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(bytes),
            Body::Stream {
                body,
                content_type,
                length,
            } => {
                let builder = builder.header(reqwest::header::CONTENT_TYPE, content_type);
                let builder = match length {
                    Some(len) => builder.header(reqwest::header::CONTENT_LENGTH, len),
                    None => builder,
                };
                builder.body(body)
            }
        };
        let response = builder.send().await.map_err(|e| self.network_error(&e))?;
        let status = response.status();
        if self.verbose {
            eprintln!("< {status}");
        }
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().await.unwrap_or_default();
        Err(CliError::from_response(status.as_u16(), &text))
    }

    /// Sends a request and parses the response as JSON. An empty body is `null`.
    pub async fn json(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Body,
    ) -> CliResult<Value> {
        let response = self.send(method, segments, query, body).await?;
        let bytes = response.bytes().await.map_err(|e| self.network_error(&e))?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| {
            CliError::bad_response(format!("The core sent a response that is not JSON: {e}"))
        })
    }

    /// Sends a request and returns the response body as text.
    pub async fn text(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Body,
    ) -> CliResult<String> {
        let response = self.send(method, segments, query, body).await?;
        response.text().await.map_err(|e| self.network_error(&e))
    }

    pub async fn get(&self, segments: &[&str]) -> CliResult<Value> {
        self.json(Method::GET, segments, &[], Body::Empty).await
    }

    pub async fn get_query(&self, segments: &[&str], query: &[(&str, String)]) -> CliResult<Value> {
        self.json(Method::GET, segments, query, Body::Empty).await
    }

    pub async fn put(&self, segments: &[&str], body: Body) -> CliResult<Value> {
        self.json(Method::PUT, segments, &[], body).await
    }

    pub async fn post(&self, segments: &[&str], body: Body) -> CliResult<Value> {
        self.json(Method::POST, segments, &[], body).await
    }

    pub async fn delete(&self, segments: &[&str]) -> CliResult<Value> {
        self.json(Method::DELETE, segments, &[], Body::Empty).await
    }

    /// `POST /user/login` with HTTP basic auth.
    pub async fn login(&self, username: &str, password: &str) -> CliResult<Value> {
        let url = self.api_url(&["user", "login"]);
        if self.verbose {
            eprintln!("> POST {}", url.path());
        }
        let response = self
            .client
            .post(url)
            .basic_auth(username, Some(password))
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| self.network_error(&e))?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = response.text().await.unwrap_or_default();
        if status.is_success() {
            return serde_json::from_str(&text).map_err(|e| {
                CliError::bad_response(format!("The core sent an invalid login reply: {e}"))
            });
        }
        let mut error = CliError::from_response(status.as_u16(), &text);
        if status == StatusCode::UNAUTHORIZED {
            error.message = "Wrong user name or password".to_string();
        } else if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = retry_after
                .map(|s| format!(" Try again in {s} seconds."))
                .unwrap_or_default();
            error.message = format!("Too many failed sign-in attempts.{wait}");
        }
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_urls() {
        let cases = [
            ("localhost", "https://localhost:16662/"),
            ("192.168.1.5:8000", "https://192.168.1.5:8000/"),
            ("http://localhost", "http://localhost:16662/"),
            ("https://core.example.com:443", "https://core.example.com/"),
            (
                "https://core.example.com/",
                "https://core.example.com:16662/",
            ),
            ("https://h:16662/api/v1/", "https://h:16662/"),
            ("https://h/lodestone/", "https://h:16662/lodestone"),
            ("[::1]:16662", "https://[::1]:16662/"),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_core_url(input).unwrap().as_str(), expected, "{input}");
        }
        for bad in ["", "ftp://x", "https://user:pw@h", "https://"] {
            assert!(parse_core_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn api_urls_encode_segments() {
        let conn = Connection::new(
            parse_core_url("https://h:1").unwrap(),
            None,
            Trust::System,
            false,
        )
        .unwrap();
        assert_eq!(
            conn.api_url(&["instance", "list"]).as_str(),
            "https://h:1/api/v1/instance/list"
        );
        assert_eq!(
            conn.api_url(&["instance", "u", "mods", "a b/c?.jar"])
                .as_str(),
            "https://h:1/api/v1/instance/u/mods/a%20b%2Fc%3F.jar"
        );
        let prefixed = Connection::new(
            parse_core_url("https://h:1/lodestone").unwrap(),
            None,
            Trust::System,
            false,
        )
        .unwrap();
        assert_eq!(
            prefixed.api_url(&["info"]).as_str(),
            "https://h:1/lodestone/api/v1/info"
        );
    }

    #[test]
    fn loopback_hosts() {
        for host in [
            "http://localhost",
            "http://127.0.0.1",
            "http://[::1]",
            "http://LOCALHOST",
        ] {
            assert!(is_loopback_host(&Url::parse(host).unwrap()), "{host}");
        }
        for host in [
            "http://192.168.0.2",
            "http://example.com",
            "http://[fe80::1]",
        ] {
            assert!(!is_loopback_host(&Url::parse(host).unwrap()), "{host}");
        }
    }
}
