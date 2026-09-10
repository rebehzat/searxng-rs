# searxng-rs

A Rust single-binary, agent-friendly metasearch CLI inspired by [SearXNG](https://github.com/searxng/searxng). It provides normalized results, concurrent engine requests, deduplication, JSON/text output, TOML configuration, and a JSONL protocol for automation.

> This is an independent implementation, not an official SearXNG project. It currently contains a small, working engine set; feature parity is being developed incrementally (see [`ROADMAP.md`](ROADMAP.md)).

## Build

```sh
cargo build --release
./target/release/searxng-rs search "rust programming" --format json
```

The binary has no runtime dependency on Python, Node, or a database.

## CLI

```sh
# Human-readable output
searxng-rs search "privacy search"

# Machine-readable response
searxng-rs search "rust" --format json --limit 5

# Restrict engines
searxng-rs search "rust" --engine wikipedia --timeout-secs 15

# List configured engines
searxng-rs engines

# Emit a tool schema for agent registration
searxng-rs schema
```

Results from an unavailable engine are reported in `engines[].error`; one backend failure does not discard successful results from other backends.

## Agent protocol (JSONL)

`--agent` reads one JSON object per line on stdin and emits exactly one JSON object per request on stdout. Logs and human output must not be mixed into the protocol stream.

Request:

```json
{"id":"job-1","query":"rust async","engines":["wikipedia"],"limit":5,"timeout_secs":10}
```

Response shape:

```json
{"id":"job-1","query":"rust async","ok":true,"error":null,"results":[{"engine":"wikipedia","title":"...","url":"...","snippet":"...","rank":1,"score":1.0,"metadata":{"language":"en"}}],"engines":[{"name":"wikipedia","ok":true,"count":1,"error":null}],"elapsed_ms":123}
```

Example:

```sh
printf '%s\n' '{"id":1,"query":"Rust"}' | searxng-rs --agent
```

This protocol is suitable for OpenClaw, pi, shell agents, and other tool runners that can launch a process and exchange JSONL. See [`AGENTS.md`](AGENTS.md) for integration guidance.

## Configuration

`searxng-rs` loads `--config PATH`, then `./searxng-rs.toml`, then built-in defaults. Start from [`searxng-rs.example.toml`](searxng-rs.example.toml).

```toml
[settings]
timeout_secs = 10
max_concurrent = 4
user_agent = "my-search-agent/0.1 (+https://example.invalid/contact)"

[engines.wikipedia]
type = "wikipedia"
language = "en"

[engines.ddg_html]
type = "duckduckgo_html"
region = "wt-wt"

# Documented JSON APIs can be configured without recompiling.
[engines.example_api]
type = "json_api"
endpoint = "https://api.example.invalid/search"
query_param = "q"
api_key_env = "EXAMPLE_API_KEY"
api_key_header = "Authorization"
```

## Responsible fetching

Adapters use documented/public endpoints, identify themselves with a configurable User-Agent, honor timeouts, and surface blocks or CAPTCHA pages as errors. This project does **not** solve CAPTCHAs, spoof browser fingerprints, rotate IPs, defeat rate limits, or bypass access controls. Operators are responsible for applicable laws, provider terms, and reasonable request rates.

## License and attribution

Licensed under AGPL-3.0-or-later. See [`LICENSE`](LICENSE). Inspired by and interoperable with concepts from SearXNG; this repository is not affiliated with the SearXNG maintainers.
