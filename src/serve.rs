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

use std::sync::Arc;

use anyhow::Result;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::run_search;

/// Maximum bytes we are willing to read for a request head.
const MAX_HEAD: usize = 16 * 1024;

/// Serve SearXNG-compatible JSON search on `bind:port` until stopped.
pub async fn run(config: Config, bind: String, port: u16) -> Result<()> {
    let config = Arc::new(config);
    let listener = TcpListener::bind((bind.as_str(), port)).await?;
    info!("listening on http://{}:{}/search", bind, port);
    loop {
        let (stream, peer) = listener.accept().await?;
        debug!(%peer, "connection");
        // Sequential on purpose: this server is a localhost companion for
        // agents, and each request already fans out to engines concurrently.
        if let Err(error) = handle_connection(stream, Arc::clone(&config)).await {
            warn!(%peer, %error, "connection failed");
        }
    }
}

async fn handle_connection(mut stream: TcpStream, config: Arc<Config>) -> Result<()> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    // Read until end of headers; we do not support request bodies.
    let head = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.len() > MAX_HEAD {
            respond(&mut stream, 431, r#"{"error":"headers too large"}"#).await?;
            return Ok(());
        }
        if let Some(pos) = find_head_end(&buf) {
            break String::from_utf8_lossy(&buf[..pos]).into_owned();
        }
    };

    let Some(path) = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
    else {
        respond(&mut stream, 400, r#"{"error":"bad request"}"#).await?;
        return Ok(());
    };
    debug!(%path, "request");

    let Some((route, query)) = path.split_once('?') else {
        respond(&mut stream, 404, r#"{"error":"not found"}"#).await?;
        return Ok(());
    };
    if route != "/search" {
        respond(&mut stream, 404, r#"{"error":"not found"}"#).await?;
        return Ok(());
    }
    let params = parse_query(query);
    let Some(q) = params.iter().find(|(k, _)| k == "q").map(|(_, v)| v) else {
        respond(&mut stream, 400, r#"{"error":"missing q"}"#).await?;
        return Ok(());
    };

    let limit = params
        .iter()
        .find(|(k, _)| k == "limit")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .map(|l| l.clamp(1, 100))
        .unwrap_or(10);

    let response = run_search(config.as_ref(), q, None, limit, None).await;
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
    respond(&mut stream, 200, &serde_json::to_string(&body)?).await
}

/// Locate `\r\n\r\n` separating head from body.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Minimal `application/x-www-form-urlencoded` query parser.
fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (
                urlencoding::decode(k)
                    .map(|v| v.into_owned())
                    .unwrap_or_default(),
                urlencoding::decode(v)
                    .map(|v| v.into_owned())
                    .unwrap_or_default(),
            ),
            None => (
                urlencoding::decode(pair)
                    .map(|v| v.into_owned())
                    .unwrap_or_default(),
                String::new(),
            ),
        })
        .collect()
}

async fn respond(stream: &mut TcpStream, status: u16, body: &str) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}
