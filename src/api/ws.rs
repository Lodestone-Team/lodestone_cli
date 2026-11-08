//! WebSocket connections to the core's live streams (console, events, monitor), with the
//! same certificate checks as the REST client.
//!
//! Browsers cannot set headers on WebSockets, so the core takes the token from the query
//! string, as the dashboard sends it. URLs with a token are never printed.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::WebSocketStream;

use super::error::{kind, CliError, CliResult};
use super::http::Connection;
use super::tls;

/// A plain or TLS stream.
pub enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

pub type Socket = WebSocketStream<Stream>;

/// Opens a WebSocket to an API route. `query` is added to the URL as is; pass the token in
/// it where the route wants one.
pub async fn connect(
    conn: &Connection,
    segments: &[&str],
    query: &[(&str, String)],
) -> CliResult<Socket> {
    let mut url = conn.api_url(segments);
    if !query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
    }
    let tcp = tls::connect_tcp(&url).await?;
    let stream = if url.scheme() == "https" {
        Stream::Tls(Box::new(
            tls::handshake(conn.tls_config.clone(), &url, tcp).await?,
        ))
    } else {
        Stream::Plain(tcp)
    };
    let ws_scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(ws_scheme)
        .expect("ws and wss are valid schemes for http URLs");
    let handshake = tokio_tungstenite::client_async(url.as_str(), stream);
    match tokio::time::timeout(tls::CONNECT_TIMEOUT, handshake).await {
        Ok(Ok((socket, _response))) => Ok(socket),
        Ok(Err(tungstenite::Error::Http(response))) => {
            let status = response.status().as_u16();
            let body = response
                .body()
                .as_deref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            Err(CliError::from_response(status, &body))
        }
        Ok(Err(e)) => Err(CliError::new(
            kind::NETWORK,
            format!("WebSocket connection failed: {e}"),
        )),
        Err(_) => Err(CliError::new(
            kind::NETWORK,
            "Timed out opening the WebSocket",
        )),
    }
}

/// The `token` query parameter of the console and monitor streams: `Bearer <jwt>`.
pub fn bearer_query(conn: &Connection) -> CliResult<(&'static str, String)> {
    let token = conn
        .token()
        .ok_or_else(|| CliError::new(kind::NOT_LOGGED_IN, "Not logged in"))?;
    Ok(("token", format!("Bearer {token}")))
}

/// What reading the next message gave.
pub enum Next {
    Json(Value),
    /// The connection closed or broke; reconnect if following.
    Closed(Option<String>),
}

/// Reads the next JSON text message, skipping other frames.
pub async fn next_json(socket: &mut Socket) -> Next {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => match serde_json::from_str(text.as_str()) {
                Ok(value) => return Next::Json(value),
                // the core only sends JSON; skip anything else
                Err(_) => continue,
            },
            Some(Ok(Message::Close(frame))) => {
                return Next::Closed(frame.map(|f| f.reason.to_string()))
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Next::Closed(Some(e.to_string())),
            None => return Next::Closed(None),
        }
    }
}

/// Backoff between reconnect attempts: 1 s doubling up to 30 s.
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    pub fn new() -> Self {
        Self { failures: 0 }
    }

    pub fn reset(&mut self) {
        self.failures = 0;
    }

    pub fn next_delay(&mut self) -> Duration {
        let seconds = 1u64 << self.failures.min(5);
        self.failures += 1;
        Duration::from_secs(seconds.min(30))
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether an error is worth reconnecting after: network trouble, not a refusal.
pub fn is_retryable(e: &CliError) -> bool {
    e.kind == kind::NETWORK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_thirty_seconds() {
        let mut backoff = Backoff::new();
        let delays: Vec<u64> = (0..8).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30, 30]);
        backoff.reset();
        assert_eq!(backoff.next_delay().as_secs(), 1);
    }
}
