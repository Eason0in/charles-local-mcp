# charles-local-mcp

Local-only, profile-driven Charles Proxy automation for macOS. The same
application service is exposed through a JSON CLI and an MCP stdio server.

Version `0.1.3` supports Charles `4.6.8` on macOS. The public package contains
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
charles-local-mcp --profiles-file profiles.toml --evidence-root /absolute/path/exports session evidence --profile demo --xml-file session.xml --json
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

## Local XML session evidence

`session evidence` reads only a selected local `.xml` file inside the
configured `--evidence-root` and the selected immutable profile. If the option
is omitted, the root defaults to `<state-dir>/evidence`; it is resolved only
when evidence analysis is requested, so existing setup and inspection commands
do not require that directory. Relative traversal and absolute paths outside
the root are rejected without returning local paths. The evidence root, parent
directories, and selected file must not be symbolic links.

The operation uses the selected profile's exact `sourceHost` only as an
internal filter. The bundle returns the profile name with `hostScope: "exact"`,
a fixed HTTP method enum, per-bundle opaque route references such as
`route-001`, query parameter counts, numeric status/duration, and
request/response sizes when those attributes exist. Repeated raw paths receive
the same opaque route reference within one bundle. Other hosts are excluded and
reported only as a count.

The operation never returns raw hosts, paths, query names, query values,
headers, cookies, credentials, or bodies. Unknown request methods become the
fixed value `OTHER`. It does not upload or persist the source session and does
not write an evidence ledger.
Files are limited to 10 MiB and 1,000 transactions; output is limited to 100
requests and 50 failures. DTD, DOCTYPE, and entity references are rejected.
Some Charles exports include a DOCTYPE declaration; make a trusted local copy
and remove the complete declaration before analysis. See
[`docs/session-evidence.md`](docs/session-evidence.md) for the contract, safety
boundary, limits, and stable error codes.

The real-device procedure is intentionally manual: see
[`docs/manual-smoke.md`](docs/manual-smoke.md).

## Install

Build from crates.io when a local Rust toolchain is available:

```console
cargo install charles-local-mcp --locked
```

For a one-click MCP client installation on macOS, use the universal `.mcpb`
asset from the matching GitHub Release. The native binary and bundle are not signed or notarized by Apple, so macOS may show a security warning when opening a downloaded asset. Verify `SHA256SUMS` from that release before installing. The bundle asks for a TOML profiles file and starts `charles-local-mcp serve`; profiles remain read-only to MCP tools.
It also asks for the XML evidence folder that is the only directory the
`session_evidence` tool may read.

Release maintainers should follow [`docs/releasing.md`](docs/releasing.md).
