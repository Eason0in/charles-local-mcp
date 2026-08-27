# Local Charles XML session evidence

`session evidence` is a read-only operation shared by the JSON CLI and MCP
server. It analyzes one Charles XML export that the user explicitly selects and
uses one immutable TOML profile as an exact host allowlist.

```console
charles-local-mcp \
  --profiles-file profiles.toml \
  --evidence-root /absolute/path/exports \
  session evidence \
  --profile demo \
  --xml-file session.xml \
  --json
```

The MCP tool is `session_evidence` and accepts the same `profile` and `xmlFile`
arguments. `xmlFile` may be relative or absolute, but its canonical target must
stay inside the evidence root selected when the server starts. MCPB installs
ask the user to select that directory. Without `--evidence-root`, the root
defaults to `<state-dir>/evidence` and is checked only when this operation runs.

## Evidence contract

The response's `data` object contains:

- `schemaVersion`: `charles-session-evidence/v1`.
- `source`: identifies a local-only `charles_xml_export`; the local file path is
  not returned.
- `profile`: the selected profile name plus `hostScope: "exact"`; the configured
  host value is used only as an internal filter and is not returned.
- `summary`: total, included, excluded-host, failure, 4xx, 5xx, and omitted-item
  counts.
- `requests`: at most 100 allowed-host request summaries.
- `failures`: at most 50 allowed-host 4xx/5xx summaries.

Each request summary contains only a fixed HTTP method enum, a per-bundle opaque
route reference such as `route-001`, query parameter count, numeric status when
available, duration when available, and request/response size when the XML
export contains those attributes. Repeated raw paths receive the same route
reference within one bundle. Unknown methods become the fixed value `OTHER`.
Raw hosts, paths, query names, and query values are never returned.

## Safety boundary

- Only a local `.xml` file whose canonical target stays within the configured
  evidence root is read. Relative traversal and symlink escapes are rejected.
- The file is never uploaded, persisted, copied into state, or written to a
  ledger.
- Only transactions whose host exactly matches the selected immutable
  profile's `sourceHost` are included. Other host names are never returned.
- Hosts, paths, query names, query values, non-whitelisted method strings,
  headers, cookies, authorization values, request bodies, response bodies,
  client addresses, and remote addresses are never returned.
- Files larger than 10 MiB, more than 1,000 transactions, paths/query fields
  larger than 2 KiB, and more than 50 query parameters are rejected.
- DTD, DOCTYPE, and general entity references are rejected before evidence is
  returned. The parser never resolves external entities.

The [Charles export documentation](https://www.charlesproxy.com/documentation/using-charles/export/)
describes XML as its third-party interchange format. Some exports include a
DOCTYPE declaration. To preserve the fail-closed parser boundary, create a
trusted local copy and remove the complete DOCTYPE declaration before running
this operation. Never weaken the parser or enable entity resolution for an
untrusted export.

## Stable errors

| Code | Meaning |
| --- | --- |
| `invalid_profile` | The profiles file or selected profile is invalid. |
| `session_file_not_xml` | The selected path is not an `.xml` file. |
| `evidence_root_unavailable` | The configured evidence root is not an accessible directory. |
| `session_file_outside_root` | The path traverses or resolves outside the configured root. |
| `session_read_failed` | The selected local file could not be read. |
| `session_file_too_large` | The file exceeds 10 MiB. |
| `session_xml_unsafe` | A DOCTYPE, DTD, or entity reference was found. |
| `session_xml_malformed` | The XML or required Charles structure is invalid. |
| `session_item_limit_exceeded` | The export exceeds 1,000 transactions. |
| `session_field_limit_exceeded` | A bounded field or query count exceeds its limit. |

The evidence operation does not start or control Charles, mutate a device,
read HAR/Trace files, perform AI analysis, or send data over the network.
