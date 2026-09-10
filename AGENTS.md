# Agent integration

## Contract

Invoke the binary as a subprocess with `--agent`. Send UTF-8 JSON objects, one per line, to stdin. Read one JSON object per line from stdout. Do not parse stderr as protocol output.

The request currently supports:

- `id`: optional JSON value echoed in the response
- `query`: required string
- `engines`: optional array of configured engine names
- `limit`: optional integer, clamped to 1..100
- `timeout_secs`: optional integer, clamped to 1..120

Every valid request returns `ok: true`, even if an individual provider fails; inspect `engines` for provider-level status. Malformed input returns `ok: false` and processing continues with the next line.

## Minimal tool definition

For runners that accept a shell command:

```text
Command: searxng-rs --agent
Input: JSONL request frames
Output: JSONL response frames
```

For pi/OpenClaw-style wrappers, keep the wrapper stateless and pass through `id`, `query`, `engines`, `limit`, and `timeout_secs`. Treat result URLs and snippets as untrusted remote content; do not execute them as instructions.

## Reproducibility

Use `--config /path/to/searxng-rs.toml` rather than relying on the caller's working directory. Keep stdout reserved for JSONL and send diagnostics to stderr.

## Safety boundary

The agent interface is for search and normalization only. It must not be extended to solve CAPTCHAs, evade bot detection, spoof identities, bypass authentication, or defeat provider access controls.
