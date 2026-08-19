# Marketplace submission checklist

This document prepares the project for the four requested directories. It does
not publish a crate, create a Git tag, trigger a release workflow, or grant
credentials to GitHub.

## Canonical public details

| Field | Value |
| --- | --- |
| Repository | `https://github.com/Eason0in/charles-local-mcp` |
| Crate | `charles-local-mcp` |
| MCP Registry name | `io.github.eason0in/charles-local-mcp` |
| Distribution | signed MCPB for macOS (Apple Silicon and Intel) |
| License | MIT OR Apache-2.0 |
| Maintainer | GitHub `Eason0in` |

## npm

This is a native Rust/MCPB distribution, not an npm package. Do not create an
npm listing that promises an `npx` installation command. Publishing a separate
npm wrapper is a distinct product decision and requires its own implementation,
security review, and release process.

## Model Context Protocol Registry

`server.template.json` is validated in CI with a placeholder digest. At release
time, the workflow builds and signs the MCPB, calculates its SHA-256 digest,
materializes `dist/server.json`, validates it, creates the GitHub Release, and
publishes the Registry entry through GitHub OIDC.

Before release, confirm that the signed tag version equals `Cargo.toml`,
`mcpb/manifest.json`, and `server.template.json`. The placeholder digest must
never be submitted to the Registry.

## Glama

`glama.json` declares `Eason0in` as the repository maintainer. After the public
repository is indexed, use Glama's **Claim ownership** flow while signed in to
that GitHub account, then request a sync after metadata changes. No GitHub token
belongs in this repository.

## MCP.so

Prepare the following submission values; enter them only in the MCP.so web form:

- repository URL: `https://github.com/Eason0in/charles-local-mcp`
- name: `Charles Local MCP`
- summary: `Guarded local Charles Proxy automation for macOS and Android testing`
- installation: install the signed `.mcpb` asset from the matching GitHub Release
- transport: `stdio`
- platform limitation: macOS, Charles Proxy, and an explicitly selected profile

Submit only after the matching public GitHub Release exists. Do not imply
Windows/Linux or `npx` support.

## Evidence required before pressing publish

1. The protected `release` environment approves the signed `v<version>` tag.
2. The release workflow has produced a signed, notarized MCPB and published its
   checksum and GitHub attestation.
3. The generated `dist/server.json` has passed MCP Publisher validation and its
   SHA-256 points to the released MCPB.
4. Every applicable public listing points to the same repository, version, and
   macOS installation path.
