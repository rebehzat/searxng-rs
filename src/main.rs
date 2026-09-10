//! `searxng-rs`: a small, single-binary, agent-friendly metasearch client.
//!
//! This is an independent Rust implementation with a normalized engine API.
//! It intentionally uses documented/public endpoints and does not circumvent
//! CAPTCHAs, bot protection, rate limits, or access controls.

mod config;
mod engines;
mod error;
mod models;

use std::collections::HashSet;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use futures::{StreamExt, stream};
use serde::Serialize;

use crate::config::Config;
use crate::engines::{Engine, build_engine};
use crate::models::{AgentRequest, AgentResponse, EngineStatus, SearchResponse};

#[derive(Debug, Parser)]
#[command(
    name = "searxng-rs",
    version,
    about = "Privacy-friendly, agent-compatible metasearch CLI"
)]
struct Cli {
    /// Read JSONL requests from stdin and write one JSON response per line.
    #[arg(long, global = true)]
    agent: bool,
    /// TOML configuration file. Defaults to ./searxng-rs.toml, then built-ins.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Search configured engines.
    Search(SearchArgs),
    /// List configured engines and their adapter types.
    Engines,
}

#[derive(Debug, Parser)]
struct SearchArgs {
    /// Search terms.
    query: String,
    /// Limit the number of merged results.
    #[arg(short, long, default_value_t = 10)]
    limit: usize,
    /// Restrict the request to one or more configured engine names.
    #[arg(short, long)]
    engine: Vec<String>,
    /// Per-engine request timeout in seconds.
    #[arg(long)]
    timeout_secs: Option<u64>,
    /// Output format for command-line mode.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Serialize)]
struct EngineInfo {
    name: String,
    r#type: String,
    enabled: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::resolve(cli.config.as_ref());

    if cli.agent {
        return run_agent(config).await;
    }

    match cli.command {
        Some(Command::Search(args)) => {
            let response = run_search(
                &config,
                &args.query,
                if args.engine.is_empty() {
                    None
                } else {
                    Some(args.engine.as_slice())
                },
                args.limit.clamp(1, 100),
                args.timeout_secs,
            )
            .await;
            match args.format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&response)?),
                OutputFormat::Text => print_text(&response),
            }
        }
        Some(Command::Engines) => {
            let infos: Vec<_> = config
                .engines
                .iter()
                .map(|(name, engine)| EngineInfo {
                    name: name.clone(),
                    r#type: engine.engine_type.clone(),
                    enabled: engine.enabled,
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&infos)?);
        }
        None => {
            let mut command = Cli::command();
            command.print_help()?;
            println!();
        }
    }
    Ok(())
}

async fn run_agent(config: Config) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<AgentRequest>(&line) {
            Ok(request) => {
                let query = request.query.trim().to_string();
                let limit = request.limit.unwrap_or(10).clamp(1, 100);
                let timeout = request.timeout_secs;
                let id = request.id;
                let response =
                    run_search(&config, &query, request.engines.as_deref(), limit, timeout).await;
                let frame = AgentResponse {
                    id: id.as_ref(),
                    query,
                    ok: true,
                    error: None,
                    results: response.results,
                    engines: response.engines,
                    elapsed_ms: response.elapsed_ms,
                };
                serde_json::to_writer(&mut stdout, &frame)?;
                writeln!(stdout)?;
            }
            Err(error) => {
                let frame = AgentResponse {
                    id: None,
                    query: String::new(),
                    ok: false,
                    error: Some(format!("invalid JSONL request: {error}")),
                    results: Vec::new(),
                    engines: Vec::new(),
                    elapsed_ms: 0,
                };
                serde_json::to_writer(&mut stdout, &frame)?;
                writeln!(stdout)?;
            }
        }
        stdout.flush()?;
    }
    Ok(())
}

async fn run_search(
    config: &Config,
    query: &str,
    requested: Option<&[String]>,
    limit: usize,
    timeout_override: Option<u64>,
) -> SearchResponse {
    let started = Instant::now();
    let timeout_secs = timeout_override
        .unwrap_or(config.settings.timeout_secs)
        .clamp(1, 120);
    let timeout = Duration::from_secs(timeout_secs);
    let (selected, unknown) = config.select_engines(requested);

    let mut statuses: Vec<EngineStatus> = unknown
        .into_iter()
        .map(|name| EngineStatus {
            name,
            ok: false,
            count: 0,
            error: Some("unknown or disabled engine".into()),
        })
        .collect();
    let mut built: Vec<(String, Arc<dyn Engine>)> = Vec::new();
    for (name, engine_config) in selected {
        match build_engine(&name, &engine_config, &config.settings.user_agent) {
            Ok(engine) => built.push((name, Arc::from(engine))),
            Err(error) => statuses.push(EngineStatus {
                name,
                ok: false,
                count: 0,
                error: Some(error.to_string()),
            }),
        }
    }

    let concurrency = config.settings.max_concurrent.max(1);
    let mut responses = stream::iter(built.into_iter().map(|(name, engine)| async move {
        let result = engine.search(query, limit, timeout).await;
        (name, result)
    }))
    .buffer_unordered(concurrency)
    .collect::<Vec<_>>()
    .await;
    responses.sort_by(|a, b| a.0.cmp(&b.0));

    let mut merged = Vec::new();
    let mut seen_urls = HashSet::new();
    for (name, outcome) in responses {
        match outcome {
            Ok(results) => {
                let count = results.len();
                statuses.push(EngineStatus {
                    name,
                    ok: true,
                    count,
                    error: None,
                });
                for result in results {
                    if seen_urls.insert(result.url.clone()) {
                        merged.push(result);
                    }
                }
            }
            Err(error) => statuses.push(EngineStatus {
                name,
                ok: false,
                count: 0,
                error: Some(error.to_string()),
            }),
        }
    }

    merged.truncate(limit);
    for (index, result) in merged.iter_mut().enumerate() {
        result.rank = index + 1;
        result.score = 1.0 / (index as f64 + 1.0);
    }
    statuses.sort_by(|a, b| a.name.cmp(&b.name));

    SearchResponse {
        query: query.to_string(),
        results: merged,
        engines: statuses,
        elapsed_ms: started.elapsed().as_millis(),
    }
}

fn print_text(response: &SearchResponse) {
    for result in &response.results {
        println!("{}. {} [{}]", result.rank, result.title, result.engine);
        println!("   {}", result.url);
        if let Some(snippet) = &result.snippet {
            println!("   {}", snippet);
        }
        println!();
    }
    eprintln!(
        "{} result(s); engines: {}",
        response.results.len(),
        response.engines.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_accepts_agent_flag() {
        let cli = Cli::try_parse_from(["searxng-rs", "--agent"]).unwrap();
        assert!(cli.agent);
    }

    #[test]
    fn cli_accepts_json_search() {
        let cli =
            Cli::try_parse_from(["searxng-rs", "search", "rust", "--format", "json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Search(SearchArgs {
                format: OutputFormat::Json,
                ..
            }))
        ));
    }
}
