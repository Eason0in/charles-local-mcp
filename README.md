# charles-local-mcp

Local-only, profile-driven Charles Proxy automation for macOS. The same
application service is exposed through a JSON CLI and an MCP stdio server.

Version `0.1.0` supports Charles `4.6.8` on macOS. The public package contains
only generic `example.com` fixtures and no organization-specific hosts or paths.

## Profile

```toml
schemaVersion = 1

[profiles.demo]
sourceHost = "app.example.com"
sourcePath = "/api*" # optional Charles path pattern
destinationUrl = "http://127.0.0.1:8080"
sslHosts = ["app.example.com"]
verificationUrl = "https://app.example.com/health"
```

Omit `destinationUrl` for a proxy-only profile that disables Map Remote while
retaining the exact SSL hosts and optional verification URL. When present,
`destinationUrl` is restricted to loopback addresses.

Map Remote destinations must resolve to a loopback host. `verificationUrl` is
optional, must use HTTPS without embedded credentials, and must match
`sourceHost`.

## Commands

```console
charles-local-mcp doctor --json
charles-local-mcp --profiles-file profiles.toml profiles validate --json
charles-local-mcp --profiles-file profiles.toml setup plan --profile demo --platform android --json
charles-local-mcp --profiles-file profiles.toml setup apply --token TOKEN --json
charles-local-mcp setup resume --token TOKEN --json
charles-local-mcp status --json
charles-local-mcp cleanup plan --json
charles-local-mcp cleanup apply --token TOKEN --json
charles-local-mcp serve
```

State defaults to `~/Library/Application Support/charles-local-mcp`. Tests and
integrators can isolate it with `CHARLES_LOCAL_MCP_HOME` or `--state-dir`.
CLI stdout is JSON only; MCP stdout is protocol only. Diagnostics use stderr.

Mutation commands require macOS and Charles `4.6.8`. Setup and cleanup plans
expire after 15 minutes, are single-use, and are rejected if state changed.
Only one active session is allowed in a state directory.

The real-device procedure is intentionally manual: see
[`docs/manual-smoke.md`](docs/manual-smoke.md).

## Install

Build from crates.io when a local Rust toolchain is available:

```console
cargo install charles-local-mcp --locked
```

For a one-click MCP client installation on macOS, use the signed and notarized
universal `.mcpb` asset from the matching GitHub Release. The bundle asks for a
TOML profiles file and starts `charles-local-mcp serve`; profiles remain
read-only to MCP tools.

Release maintainers should follow [`docs/releasing.md`](docs/releasing.md).
