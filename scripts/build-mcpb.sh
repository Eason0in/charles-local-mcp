#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
rust_host="$(rustc -vV | sed -n 's/^host: //p')"
case "$rust_host" in
  aarch64-apple-darwin) asset_target="arm64-apple-darwin" ;;
  x86_64-apple-darwin) asset_target="x86_64-apple-darwin" ;;
  *)
    echo "unsupported Rust host target: $rust_host" >&2
    exit 1
    ;;
esac
output="${1:-$repo_root/dist/charles-local-mcp-0.1.0-$asset_target.mcpb}"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

cd "$repo_root"
cargo build --release --locked
mkdir -p "$stage/server" "$(dirname "$output")"
cp mcpb/manifest.json "$stage/manifest.json"
cp target/release/charles-local-mcp "$stage/server/charles-local-mcp"
chmod 0755 "$stage/server/charles-local-mcp"
npx --yes @anthropic-ai/mcpb@2.1.2 validate "$stage/manifest.json"
npx --yes @anthropic-ai/mcpb@2.1.2 pack "$stage" "$output"
npx --yes @anthropic-ai/mcpb@2.1.2 info "$output"
