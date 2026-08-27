# Releasing

Releases are intentionally explicit. CI tests the minimum Rust version, stable
Rust, Intel macOS, Apple Silicon macOS, the crates.io package, MCP stdio, MCPB
metadata, and RustSec advisories before a release can start.

## Required repository configuration

Create a protected `release` environment and configure these secrets:

- `CARGO_REGISTRY_TOKEN`: crates.io token scoped to this crate.

GitHub OIDC is used for MCP Registry authentication. The universal macOS binary
and MCPB are intentionally not signed or notarized by Apple. The workflow
publishes `SHA256SUMS` and `RELEASE-NOTICE.txt`; users may see a macOS security
warning and should verify the matching release checksum before installing.

## Release procedure

1. Keep `Cargo.toml`, `mcpb/manifest.json`, and `server.template.json` on the
   same semantic version.
2. Run the full CI workflow and the real-device procedure in
   `docs/manual-smoke.md`.
3. Create and push a signed tag such as `v0.1.3`.
4. Approve the protected `release` environment.
5. Verify crates.io, the universal binary/MCPB GitHub assets, `SHA256SUMS`,
   `RELEASE-NOTICE.txt`, attestations, and the MCP Registry entry.

The workflow refuses a tag whose version differs from any release manifest.

The account-side setup and submission data for npm, the MCP Registry, Glama,
and MCP.so are tracked in [Marketplace submission checklist](marketplace-submission.md).
