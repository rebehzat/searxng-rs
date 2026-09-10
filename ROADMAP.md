# Roadmap to SearXNG-compatible coverage

A complete rewrite cannot be safely or accurately generated in one pass. The upstream repository contains hundreds of independently maintained engines plus settings, plugins, themes, localization, answerers, caching, rate limiting, and server/API behavior. This project therefore uses compatibility gates rather than claiming parity prematurely.

## Delivered in 0.1

- Single Rust binary with no Python runtime
- Concurrent normalized search pipeline
- Deduplication and stable rank/score fields
- Text and JSON output
- JSONL stdin/stdout agent protocol
- TOML engine configuration
- Wikipedia MediaWiki API adapter
- DuckDuckGo HTML adapter with block/challenge reporting
- Configurable JSON API adapter with environment-based API-key support
- Unit tests and AGPL-3.0-or-later licensing

## Planned gates

1. **Core compatibility:** query model (categories, language, safesearch, time range, page), pagination, bang handling, timeout/error taxonomy, and deterministic ranking tests.
2. **Engine SDK:** versioned adapter trait, fixture-based parser tests, rate/concurrency policy, API-key secret handling, and generated engine health report.
3. **High-value engines:** port documented/API-backed web, news, image, video, science, files, maps, and social providers in small reviewed batches.
4. **Server mode:** optional HTTP/JSON endpoint in the same executable, health and metrics endpoints, config reload, and privacy-preserving request controls.
5. **User-facing parity:** templates/static assets, preferences, localization, plugins, answerers, bang database, cache backends, and import tooling.
6. **Release parity:** cross-platform builds, reproducible archives, SBOM, signed checksums, and GitHub Releases containing binaries plus corresponding source.

Each gate requires tests against saved fixtures and explicit behavior comparisons with a pinned upstream SearXNG version. No provider-specific anti-bot circumvention is a compatibility target.
