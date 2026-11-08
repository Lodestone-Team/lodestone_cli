//! A streamed `multipart/form-data` body for file uploads. Files are read in chunks, so
//! uploads of any size use little memory, and the total length is sent up front so the
//! core can report progress.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use futures_util::stream::{self, Stream, StreamExt};
use tokio::io::AsyncReadExt;

use super::error::{CliError, CliResult};
use super::http::Body;

const CHUNK_SIZE: usize = 256 * 1024;

/// A file to upload: the local path and the name to send.
#[derive(Debug, Clone)]
pub struct Part {
    pub path: PathBuf,
    pub file_name: String,
    pub size: u64,
}

impl Part {
    /// A part for a local file, sent under its own name.
    pub fn for_file(path: &Path) -> CliResult<Self> {
        let metadata = std::fs::metadata(path)
            .map_err(|e| CliError::io(format!("Cannot read {}", path.display()), e))?;
        if !metadata.is_file() {
            return Err(CliError::usage(format!("{} is not a file", path.display())));
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                CliError::usage(format!("{} has no valid UTF-8 file name", path.display()))
            })?
            .to_string();
        Ok(Self {
            path: path.to_path_buf(),
            file_name,
            size: metadata.len(),
        })
    }
}

/// Escapes a file name for the `filename` parameter, as browsers do.
fn escape_file_name(name: &str) -> String {
    name.replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn part_header(boundary: &str, field: &str, file_name: &str) -> String {
    format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n",
        escape_file_name(file_name)
    )
}

fn closing(boundary: &str) -> String {
    format!("--{boundary}--\r\n")
}

/// A boundary that will not appear in the files: 32 random hex digits.
fn new_boundary() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    let rng = ring::rand::SystemRandom::new();
    if rng.fill(&mut bytes).is_err() {
        // fall back to the clock; the boundary only needs to be unlikely in the data
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        bytes.copy_from_slice(&nanos.to_le_bytes());
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("lodestone-cli-{hex}")
}

/// The length of the whole body.
fn body_length(boundary: &str, field: &str, parts: &[Part]) -> u64 {
    parts
        .iter()
        .map(|p| part_header(boundary, field, &p.file_name).len() as u64 + p.size + 2)
        .sum::<u64>()
        + closing(boundary).len() as u64
}

enum ReadState {
    Start(Part),
    Reading {
        file: tokio::fs::File,
        sent: u64,
        expected: u64,
    },
    Done,
}

/// The contents of one file, in chunks. Fails if the file's size changes while it is read,
/// since the body length was already sent.
fn file_stream(
    part: Part,
    on_chunk: impl Fn(u64) + Clone + Send + Sync + 'static,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send {
    stream::unfold(ReadState::Start(part), move |mut state| {
        let on_chunk = on_chunk.clone();
        async move {
            loop {
                match state {
                    ReadState::Done => return None,
                    ReadState::Start(part) => match tokio::fs::File::open(&part.path).await {
                        Ok(file) => {
                            state = ReadState::Reading {
                                file,
                                sent: 0,
                                expected: part.size,
                            }
                        }
                        Err(e) => return Some((Err(e), ReadState::Done)),
                    },
                    ReadState::Reading {
                        mut file,
                        sent,
                        expected,
                    } => {
                        let mut buffer = vec![0u8; CHUNK_SIZE];
                        return match file.read(&mut buffer).await {
                            Ok(0) if sent == expected => None,
                            Ok(0) => Some((
                                Err(std::io::Error::other(
                                    "the file got shorter while it was uploaded",
                                )),
                                ReadState::Done,
                            )),
                            Ok(n) if sent + n as u64 > expected => Some((
                                Err(std::io::Error::other("the file grew while it was uploaded")),
                                ReadState::Done,
                            )),
                            Ok(n) => {
                                buffer.truncate(n);
                                on_chunk(n as u64);
                                Some((
                                    Ok(Bytes::from(buffer)),
                                    ReadState::Reading {
                                        file,
                                        sent: sent + n as u64,
                                        expected,
                                    },
                                ))
                            }
                            Err(e) => Some((Err(e), ReadState::Done)),
                        };
                    }
                }
            }
        }
    })
}

/// A multipart body with every part in the form field `field`. `on_chunk` is called with
/// the size of each file chunk sent, for progress.
pub fn body(
    field: &str,
    parts: Vec<Part>,
    on_chunk: impl Fn(u64) + Clone + Send + Sync + 'static,
) -> Body {
    let boundary = new_boundary();
    let length = body_length(&boundary, field, &parts);
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let mut streams: Vec<std::pin::Pin<Box<dyn Stream<Item = std::io::Result<Bytes>> + Send>>> =
        Vec::new();
    for part in parts {
        let header = Bytes::from(part_header(&boundary, field, &part.file_name));
        streams.push(Box::pin(stream::once(async move { Ok(header) })));
        streams.push(Box::pin(file_stream(part, on_chunk.clone())));
        streams.push(Box::pin(stream::once(async {
            Ok(Bytes::from_static(b"\r\n"))
        })));
    }
    let end = Bytes::from(closing(&boundary));
    streams.push(Box::pin(stream::once(async move { Ok(end) })));
    Body::Stream {
        body: reqwest::Body::wrap_stream(stream::iter(streams).flatten()),
        content_type,
        length: Some(length),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_escaped() {
        assert_eq!(escape_file_name("a\"b\r\n.jar"), "a%22b%0D%0A.jar");
        assert_eq!(escape_file_name("服务器.jar"), "服务器.jar");
    }

    #[tokio::test]
    async fn body_matches_its_length() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jar");
        let b = dir.path().join("b c.txt");
        std::fs::write(&a, vec![7u8; CHUNK_SIZE + 10]).unwrap();
        std::fs::write(&b, b"hello").unwrap();
        let parts = vec![Part::for_file(&a).unwrap(), Part::for_file(&b).unwrap()];
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = sent.clone();
        let Body::Stream {
            body,
            content_type,
            length,
        } = body("file", parts, move |n| {
            counter.fetch_add(n, std::sync::atomic::Ordering::SeqCst);
        })
        else {
            panic!("expected a stream body");
        };
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap()
            .to_string();
        let bytes = http_body_bytes(body).await;
        assert_eq!(bytes.len() as u64, length.unwrap());
        assert_eq!(
            sent.load(std::sync::atomic::Ordering::SeqCst),
            CHUNK_SIZE as u64 + 15
        );
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.jar\"\r\n"
        )));
        assert!(text.contains(
            "filename=\"b c.txt\"\r\nContent-Type: application/octet-stream\r\n\r\nhello\r\n"
        ));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    async fn http_body_bytes(body: reqwest::Body) -> Vec<u8> {
        use http_body_util::BodyExt;
        body.collect().await.unwrap().to_bytes().to_vec()
    }
}
