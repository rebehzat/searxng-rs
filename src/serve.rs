//! Minimal HTTP server exposing a SearXNG-compatible JSON API.
//!
//! Serves `GET /search?q=<query>&format=json` with the subset of the
//! SearXNG response shape that consumers (e.g. the OpenClaw SearXNG
//! provider) actually read: `query`, `number_of_results`, and `results`
//! with `title`, `url`, `engine`, `content`.
//!
//! Deliberately dependency-free: a hand-rolled HTTP/1.1 responder over
//! `tokio::net::TcpListener`, one connection per request, no TLS. Bind it
//! to loopback or put a reverse proxy in front; this server does neither.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::LocalSet;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::run_search;

/// Maximum bytes we are willing to read for a request head.
const MAX_HEAD: usize = 16 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);

/// Serve SearXNG-compatible JSON search on `bind:port` until stopped.
pub async fn run(config: Config, bind: String, port: u16) -> Result<()> {
    let config = Arc::new(config);
    let listener = TcpListener::bind((bind.as_str(), port)).await?;
    info!("listening on http://{}:{}/search", bind, port);
    let local = LocalSet::new();
    local
        .run_until(async {
            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(connection) => connection,
                    Err(error) => {
                        warn!(%error, "accept failed");
                        continue;
                    }
                };
                debug!(%peer, "connection");
                let config = Arc::clone(&config);
                local.spawn_local(async move {
                    if let Err(error) = handle_connection(stream, config).await {
                        warn!(%peer, %error, "connection failed");
                    }
                });
            }
        })
        .await;
    Ok(())
}

async fn handle_connection(mut stream: TcpStream, config: Arc<Config>) -> Result<()> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    // Read until end of headers; we do not support request bodies.
    let head = timeout(HEAD_TIMEOUT, async {
        loop {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                return Ok::<_, std::io::Error>(HeadRead::Eof);
            }
            buf.extend_from_slice(&chunk[..read]);
            if buf.len() > MAX_HEAD {
                return Ok(HeadRead::TooLarge);
            }
            if let Some(pos) = find_head_end(&buf) {
                let head = std::str::from_utf8(&buf[..pos]).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "request head is not UTF-8",
                    )
                })?;
                return Ok(HeadRead::Complete(head.to_owned()));
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("request head timed out"))??;
    let head = match head {
        HeadRead::Eof => return Ok(()),
        HeadRead::TooLarge => {
            respond(
                &mut stream,
                431,
                r#"{"error":"headers too large"}"#.to_owned(),
            )
            .await?;
            return Ok(());
        }
        HeadRead::Complete(head) => head,
    };

    let Some((method, path)) = parse_request_start(&head) else {
        respond(&mut stream, 400, r#"{"error":"bad request"}"#.to_owned()).await?;
        return Ok(());
    };
    if method != "GET" {
        respond_with_headers(
            &mut stream,
            405,
            r#"{"error":"method not allowed"}"#.to_owned(),
            "Allow: GET\r\n".to_owned(),
        )
        .await?;
        return Ok(());
    }
    let path = path.to_owned();
    debug!(%path, "request");

    let Some((route, query)) = path.split_once('?') else {
        respond(&mut stream, 404, r#"{"error":"not found"}"#.to_owned()).await?;
        return Ok(());
    };
    if route != "/search" {
        respond(&mut stream, 404, r#"{"error":"not found"}"#.to_owned()).await?;
        return Ok(());
    }
    let params = match parse_query(query) {
        Ok(params) => params,
        Err(()) => {
            respond(
                &mut stream,
                400,
                r#"{"error":"invalid query encoding"}"#.to_owned(),
            )
            .await?;
            return Ok(());
        }
    };
    let Some(q) = params.iter().find(|(k, _)| k == "q").map(|(_, v)| v) else {
        respond(&mut stream, 400, r#"{"error":"missing q"}"#.to_owned()).await?;
        return Ok(());
    };
    let q = q.to_owned();
    if q.is_empty() {
        respond(&mut stream, 400, r#"{"error":"empty q"}"#.to_owned()).await?;
        return Ok(());
    }

    let limit = match params.iter().find(|(k, _)| k == "limit") {
        Some((_, value)) => match value.parse::<usize>() {
            Ok(limit) if (1..=100).contains(&limit) => limit,
            _ => {
                respond(&mut stream, 400, r#"{"error":"invalid limit"}"#.to_owned()).await?;
                return Ok(());
            }
        },
        None => 10,
    };

    let response = run_search(config.as_ref(), &q, None, limit, None).await;
    let body = json!({
        "query": response.query,
        "number_of_results": response.results.len(),
        "results": response
            .results
            .iter()
            .map(|r| json!({
                "title": r.title,
                "url": r.url,
                "engine": r.engine,
                "content": r.snippet.clone().unwrap_or_default(),
            }))
            .collect::<Vec<_>>(),
    });
    respond(&mut stream, 200, serde_json::to_string(&body)?).await
}

fn parse_request_start(head: &str) -> Option<(&str, &str)> {
    let mut parts = head.lines().next()?.split_whitespace();
    Some((parts.next()?, parts.next()?))
}

enum HeadRead {
    Eof,
    Complete(String),
    TooLarge,
}

/// Locate `\r\n\r\n` separating head from body.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Minimal `application/x-www-form-urlencoded` query parser.
fn parse_query(query: &str) -> Result<Vec<(String, String)>, ()> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => Ok((decode_form_component(key)?, decode_form_component(value)?)),
            None => Ok((decode_form_component(pair)?, String::new())),
        })
        .collect()
}

fn decode_form_component(value: &str) -> Result<String, ()> {
    Ok(urlencoding::decode(&value.replace('+', " "))
        .map_err(|_| ())?
        .into_owned())
}

async fn respond(stream: &mut TcpStream, status: u16, body: String) -> Result<()> {
    respond_with_headers(stream, status, body, String::new()).await
}

async fn respond_with_headers(
    stream: &mut TcpStream,
    status: u16,
    body: String,
    extra_headers: String,
) -> Result<()> {
    let reason = status_reason(status);
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n",
        body.len()
    );
    let mut head_bytes = VecDeque::from(head.into_bytes());
    stream.write_all_buf(&mut head_bytes).await?;
    let mut body_bytes = VecDeque::from(body.into_bytes());
    stream.write_all_buf(&mut body_bytes).await?;
    stream.flush().await?;
    Ok(())
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_utf8_request_heads() {
        let mut buf = b"GET /search?q=".to_vec();
        buf.push(0xff);
        buf.extend_from_slice(b" HTTP/1.1\r\n\r\n");
        let pos = find_head_end(&buf).unwrap();
        assert!(std::str::from_utf8(&buf[..pos]).is_err());
    }

    #[test]
    fn parses_form_encoded_query_values() {
        let params = parse_query("q=rust+language&lang=en%2DUS").unwrap();
        assert_eq!(
            params,
            vec![
                ("q".into(), "rust language".into()),
                ("lang".into(), "en-US".into())
            ]
        );
        assert!(parse_query("q=%FF").is_err());
    }

    #[test]
    fn parses_method_and_request_target() {
        assert_eq!(
            parse_request_start("GET /search?q=rust HTTP/1.1\r\n"),
            Some(("GET", "/search?q=rust"))
        );
        assert_eq!(
            parse_request_start("POST /search?q=rust HTTP/1.1\r\n"),
            Some(("POST", "/search?q=rust"))
        );
        assert_eq!(parse_request_start("GET\r\n"), None);
    }

    #[test]
    fn response_reason_includes_method_not_allowed() {
        assert_eq!(status_reason(405), "Method Not Allowed");
    }
}
