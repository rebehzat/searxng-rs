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

Built-in providers currently include DuckDuckGo, Wikipedia, OpenAlex, Crossref, Semantic Scholar, Stack Overflow, Open Library, Internet Archive, GitHub repositories, Hacker News, GitLab, npm, Google Books, and TVMaze. Results from an unavailable engine are reported in `engines[].error`; one backend failure does not discard successful results from other backends.

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

### Adapter options

`json_api` and `html_scrape` share these optional keys. Every one defaults to
off, so a config that sets none of them behaves exactly as before.

**Where the search term goes**

| key | default | meaning |
| --- | --- | --- |
| `query_param` | `q` | query-string parameter carrying the search term |
| `path_query` | off | put the term in the URL **path** instead; the endpoint must then contain a literal `{query}` |

`path_query` is percent-encoded to the RFC 3986 unreserved set before the URL is
parsed, so a term can never introduce a path segment, query string or fragment.
The term is not also appended as a query-string parameter.

```toml
[engines.example_path]
type = "json_api"
path_query = "true"
endpoint = "https://api.example.invalid/words/{query}"
```

**Fetching more than one page**

25 of the engines built in here have multiple upstream pages. Without the keys
below they would return only the first page, so a request for 30 results would
quietly come back short.

| key | default | meaning |
| --- | --- | --- |
| `page_param` | — | query-string parameter carrying the page number |
| `page_start` | `1` | value sent for page 1 (`0` is valid, e.g. `from=0`) |
| `page_step` | `1` | increment per page |
| `max_pages` | `3` | hard cap on requests for one search, rejected above `10` |
| `page_in_path` | off | put the page in the URL **path** instead; the endpoint must then contain a literal `{page}` |

Page *n* sends `page_start + (n - 1) × page_step`, which is the shape upstream
engines use: `first = 1 + (n-1)×35`, `from = (n-1)×10`, `offset = (n-1)×20`,
`page = n`.

```toml
[engines.example_paged]
type = "json_api"
endpoint = "https://api.example.invalid/search"
query_param = "q"
page_param = "offset"
page_start = "0"
page_step = "20"
max_pages = "3"
```

A single search stops as soon as it holds `limit` results, so an engine whose
first page already satisfies the request still makes exactly **one** request. It
also stops on a page that adds no new results, which is what protects a provider
that repeats its last page when exhausted. Results are de-duplicated by URL
across pages, and the `timeout_secs` budget covers the whole search rather than
each page.

These keys are rejected rather than silently ignored when misconfigured:
`page_param` together with `page_in_path`; either placeholder without its flag
(or a stray placeholder with paging off); `max_pages` outside `1..=10`;
`page_step = 0`; a `page_param` that collides with `query_param`/`limit_param`;
and a `page_param` already pinned in the endpoint's own query string, which
would send the page twice and never leave page 1.

**Result shaping** (`json_api`): `results_path`, `title_field`, `url_field`,
`url_prefix`, `url_template`, `url_source_field`, `fallback_url_field`,
`fallback_url_template`, `snippet_field`, `normalize_title_html`,
`normalize_snippet_html`, `snippet_max_length`, `array_value_field`,
`limit_param`, `max_limit`.

**HTML selectors** (`html_scrape`): `result_selector`, `link_selector`,
`link_url_attr`, `title_selector`, `title_attr`, `snippet_selector`, plus a
static `param_<name>` for any extra query parameter.

## Responsible fetching

Adapters use documented/public endpoints, identify themselves with a configurable User-Agent, honor timeouts, and surface blocks or CAPTCHA pages as errors. This project does **not** solve CAPTCHAs, spoof browser fingerprints, rotate IPs, defeat rate limits, or bypass access controls. Operators are responsible for applicable laws, provider terms, and reasonable request rates.

A multi-page search issues more than one request by design. It stops as soon as
it has enough results, stops on an unproductive page, and is hard-capped at
`max_pages` (default 3, never above 10) per engine per search — so raising
`max_pages` increases the load a single search puts on a provider, and
`timeout_secs` still bounds the total.

## License and attribution

Licensed under AGPL-3.0-or-later. See [`LICENSE`](LICENSE). Inspired by and interoperable with concepts from SearXNG; this repository is not affiliated with the SearXNG maintainers.
