# Releasing

Releases are intentionally explicit. CI tests the minimum Rust version, stable
Rust, Intel macOS, Apple Silicon macOS, the crates.io package, MCP stdio, MCPB
metadata, and RustSec advisories before a release can start.

## Required repository configuration

Create a protected `release` environment and configure these secrets:

- `CARGO_REGISTRY_TOKEN`: crates.io token scoped to this crate.
- `APPLE_CERTIFICATE_BASE64`: base64-encoded Developer ID Application `.p12`.
- `APPLE_CERTIFICATE_PASSWORD`: password for that `.p12`.
- `APPLE_SIGNING_IDENTITY`: its full Developer ID Application identity.
- `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`: notarization credentials.

The workflow derives the MCPB signing certificate and private key from the
Developer ID `.p12`; it never uploads either as an artifact. GitHub OIDC is used
for MCP Registry authentication.

## Release procedure

1. Keep `Cargo.toml`, `mcpb/manifest.json`, and `server.template.json` on the
   same semantic version.
2. Run the full CI workflow and the real-device procedure in
   `docs/manual-smoke.md`.
3. Create and push a signed tag such as `v0.1.0`.
4. Approve the protected `release` environment.
5. Verify crates.io, the signed and notarized universal binary/MCPB GitHub
   assets, checksums and attestations, and the MCP Registry entry.

The workflow refuses a tag whose version differs from any release manifest.

The account-side setup and submission data for npm, the MCP Registry, Glama,
and MCP.so are tracked in [Marketplace submission checklist](marketplace-submission.md).
