use std::{
    collections::BTreeMap,
    fs::File,
    io::BufReader,
    path::{Component, Path, PathBuf},
};

use quick_xml::{
    encoding::Decoder,
    events::{BytesStart, Event},
    Reader,
};
use serde::Serialize;

pub const EVIDENCE_SCHEMA_VERSION: &str = "charles-session-evidence/v1";
pub const MAX_XML_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_TRANSACTIONS: usize = 1_000;
pub const MAX_REQUESTS: usize = 100;
pub const MAX_FAILURES: usize = 50;
const MAX_ATTRIBUTE_BYTES: usize = 4 * 1024;
const MAX_PATH_BYTES: usize = 2 * 1024;
const MAX_QUERY_PARAMETERS: usize = 50;
const MAX_METHOD_BYTES: usize = 32;

#[derive(Debug)]
pub struct EvidenceError {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug, Default)]
struct RawTransaction {
    method: String,
    host: String,
    path: String,
    query: String,
    status: Option<u16>,
    duration_millis: Option<u64>,
    start_millis: Option<u64>,
    end_millis: Option<u64>,
    request_size_bytes: Option<u64>,
    response_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceRequest {
    method: &'static str,
    route_ref: String,
    query_parameter_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_millis: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_size_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceSource {
    format: &'static str,
    local_only: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceProfile<'a> {
    name: &'a str,
    host_scope: &'static str,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceSummary {
    transaction_count: usize,
    included_count: usize,
    excluded_host_count: usize,
    failure_count: usize,
    status_4xx_count: usize,
    status_5xx_count: usize,
    omitted_request_count: usize,
    omitted_failure_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceBundle<'a> {
    schema_version: &'static str,
    source: EvidenceSource,
    profile: EvidenceProfile<'a>,
    summary: EvidenceSummary,
    requests: Vec<EvidenceRequest>,
    failures: Vec<EvidenceRequest>,
}

pub fn analyze(
    evidence_root: &Path,
    xml_file: &Path,
    profile_name: &str,
    source_host: &str,
) -> Result<serde_json::Value, EvidenceError> {
    let xml_file = resolve_xml_file(evidence_root, xml_file)?;
    if !xml_file
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
    {
        return Err(EvidenceError {
            code: "session_file_not_xml",
            message: "the selected local file must use the .xml extension",
        });
    }
    let file = File::open(&xml_file).map_err(|_| EvidenceError {
        code: "session_read_failed",
        message: "unable to read the selected local XML file",
    })?;
    let metadata = file.metadata().map_err(|_| EvidenceError {
        code: "session_read_failed",
        message: "unable to inspect the selected local XML file",
    })?;
    if !metadata.is_file() {
        return Err(EvidenceError {
            code: "session_file_not_xml",
            message: "the selected local path must be an XML file",
        });
    }
    if metadata.len() > MAX_XML_BYTES {
        return Err(EvidenceError {
            code: "session_file_too_large",
            message: "the selected XML file exceeds the 10 MiB limit",
        });
    }
    let mut reader = Reader::from_reader(BufReader::new(file));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut current = None;
    let mut summary = EvidenceSummary::default();
    let mut requests = Vec::new();
    let mut failures = Vec::new();
    let mut routes = BTreeMap::new();
    let mut root_open = false;
    let mut root_closed = false;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|_| EvidenceError {
                code: "session_xml_malformed",
                message: "invalid Charles XML export",
            })?;
        match event {
            Event::DocType(_) | Event::GeneralRef(_) => {
                return Err(EvidenceError {
                    code: "session_xml_unsafe",
                    message: "DTD, DOCTYPE, and entity references are not accepted",
                });
            }
            Event::Start(start) if start.name().as_ref() == b"charles-session" => {
                if root_open || root_closed {
                    return Err(malformed());
                }
                root_open = true;
            }
            Event::Empty(start) if start.name().as_ref() == b"charles-session" => {
                if root_open || root_closed {
                    return Err(malformed());
                }
                root_closed = true;
            }
            Event::Start(start) if start.name().as_ref() == b"transaction" => {
                if !root_open || current.is_some() {
                    return Err(malformed());
                }
                current = Some(transaction(&start, reader.decoder())?);
            }
            Event::Empty(start) if start.name().as_ref() == b"transaction" => {
                return Err(malformed());
            }
            Event::Start(start) | Event::Empty(start) if start.name().as_ref() == b"request" => {
                if let Some(transaction) = current.as_mut() {
                    transaction.request_size_bytes = exchange_size(&start, reader.decoder())?;
                }
            }
            Event::Start(start) | Event::Empty(start) if start.name().as_ref() == b"response" => {
                if let Some(transaction) = current.as_mut() {
                    transaction.status = attribute(&start, b"status", reader.decoder())?
                        .and_then(|value| value.parse().ok());
                    transaction.response_size_bytes = exchange_size(&start, reader.decoder())?;
                }
            }
            Event::End(end) if end.name().as_ref() == b"transaction" => {
                let Some(transaction) = current.take() else {
                    return Err(malformed());
                };
                summary.transaction_count += 1;
                if summary.transaction_count > MAX_TRANSACTIONS {
                    return Err(EvidenceError {
                        code: "session_item_limit_exceeded",
                        message: "the XML export exceeds the 1000 transaction limit",
                    });
                }
                if transaction.host.eq_ignore_ascii_case(source_host) {
                    summary.included_count += 1;
                    let route_key = transaction
                        .path
                        .split_once('?')
                        .map_or(transaction.path.as_str(), |(path, _)| path)
                        .to_owned();
                    let next_route = routes.len() + 1;
                    let route_ref = routes
                        .entry(route_key)
                        .or_insert_with(|| format!("route-{next_route:03}"))
                        .clone();
                    let request = evidence_request(transaction, route_ref);
                    if let Some(status) = request.status {
                        if (400..=499).contains(&status) {
                            summary.status_4xx_count += 1;
                        }
                        if (500..=599).contains(&status) {
                            summary.status_5xx_count += 1;
                        }
                        if (400..=599).contains(&status) {
                            summary.failure_count += 1;
                            if failures.len() < MAX_FAILURES {
                                failures.push(request.clone());
                            } else {
                                summary.omitted_failure_count += 1;
                            }
                        }
                    }
                    if requests.len() < MAX_REQUESTS {
                        requests.push(request);
                    } else {
                        summary.omitted_request_count += 1;
                    }
                } else {
                    summary.excluded_host_count += 1;
                }
            }
            Event::End(end) if end.name().as_ref() == b"charles-session" => {
                if !root_open || current.is_some() {
                    return Err(malformed());
                }
                root_open = false;
                root_closed = true;
            }
            Event::Start(_) | Event::Empty(_) if !root_open || root_closed => {
                return Err(malformed());
            }
            Event::Eof => {
                if !root_closed || root_open || current.is_some() {
                    return Err(malformed());
                }
                break;
            }
            _ => {}
        }
        buffer.clear();
    }

    serde_json::to_value(EvidenceBundle {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        source: EvidenceSource {
            format: "charles_xml_export",
            local_only: true,
        },
        profile: EvidenceProfile {
            name: profile_name,
            host_scope: "exact",
        },
        summary,
        requests,
        failures,
    })
    .map_err(|_| EvidenceError {
        code: "operation_failed",
        message: "unable to serialize session evidence",
    })
}

fn resolve_xml_file(evidence_root: &Path, xml_file: &Path) -> Result<PathBuf, EvidenceError> {
    if xml_file
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(outside_root());
    }
    let root = evidence_root.canonicalize().map_err(|_| EvidenceError {
        code: "evidence_root_unavailable",
        message: "the configured evidence root is not an accessible directory",
    })?;
    if !root.is_dir() {
        return Err(EvidenceError {
            code: "evidence_root_unavailable",
            message: "the configured evidence root is not an accessible directory",
        });
    }
    if xml_file.is_absolute()
        && !xml_file.starts_with(evidence_root)
        && !xml_file.starts_with(&root)
    {
        return Err(outside_root());
    }
    let candidate = if xml_file.is_absolute() {
        xml_file.to_path_buf()
    } else {
        root.join(xml_file)
    };
    let candidate = candidate.canonicalize().map_err(|_| EvidenceError {
        code: "session_read_failed",
        message: "unable to read the selected local XML file",
    })?;
    if !candidate.starts_with(&root) {
        return Err(outside_root());
    }
    Ok(candidate)
}

fn outside_root() -> EvidenceError {
    EvidenceError {
        code: "session_file_outside_root",
        message: "the selected XML file must stay within the configured evidence root",
    }
}

fn transaction(start: &BytesStart<'_>, decoder: Decoder) -> Result<RawTransaction, EvidenceError> {
    let method = required_attribute(start, b"method", decoder)?;
    let host = required_attribute(start, b"host", decoder)?;
    let path = required_attribute(start, b"path", decoder)?;
    let query = attribute(start, b"query", decoder)?.unwrap_or_default();
    let path_query = path.split_once('?').map_or("", |(_, query)| query);
    let effective_query = if query.is_empty() { path_query } else { &query };
    if method.len() > MAX_METHOD_BYTES
        || host.len() > 253
        || path.len() > MAX_PATH_BYTES
        || query.len() > MAX_PATH_BYTES
        || effective_query
            .split('&')
            .filter(|part| !part.is_empty())
            .count()
            > MAX_QUERY_PARAMETERS
    {
        return Err(EvidenceError {
            code: "session_field_limit_exceeded",
            message: "a transaction field exceeds the safe evidence limit",
        });
    }
    Ok(RawTransaction {
        method,
        host,
        path,
        query,
        duration_millis: attribute(start, b"duration", decoder)?
            .and_then(|value| value.parse().ok()),
        start_millis: attribute(start, b"startTimeMillis", decoder)?
            .and_then(|value| value.parse().ok()),
        end_millis: attribute(start, b"endTimeMillis", decoder)?
            .and_then(|value| value.parse().ok()),
        ..RawTransaction::default()
    })
}

fn exchange_size(start: &BytesStart<'_>, decoder: Decoder) -> Result<Option<u64>, EvidenceError> {
    let headers: Option<u64> =
        attribute(start, b"headers", decoder)?.and_then(|value| value.parse().ok());
    let body: Option<u64> =
        attribute(start, b"body", decoder)?.and_then(|value| value.parse().ok());
    Ok(match (headers, body) {
        (Some(headers), Some(body)) => headers.checked_add(body),
        (Some(size), None) | (None, Some(size)) => Some(size),
        (None, None) => None,
    })
}

fn attribute(
    start: &BytesStart<'_>,
    name: &[u8],
    decoder: Decoder,
) -> Result<Option<String>, EvidenceError> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|_| EvidenceError {
            code: "session_xml_malformed",
            message: "invalid Charles XML export",
        })?;
        if attribute.key.as_ref() == name {
            if attribute.value.len() > MAX_ATTRIBUTE_BYTES {
                return Err(EvidenceError {
                    code: "session_field_limit_exceeded",
                    message: "a transaction field exceeds the safe evidence limit",
                });
            }
            return attribute
                .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, decoder)
                .map(|value| Some(value.into_owned()))
                .map_err(|_| EvidenceError {
                    code: "session_xml_unsafe",
                    message: "invalid Charles XML export",
                });
        }
    }
    Ok(None)
}

fn required_attribute(
    start: &BytesStart<'_>,
    name: &[u8],
    decoder: Decoder,
) -> Result<String, EvidenceError> {
    attribute(start, name, decoder)?
        .filter(|value| !value.is_empty())
        .ok_or_else(malformed)
}

fn malformed() -> EvidenceError {
    EvidenceError {
        code: "session_xml_malformed",
        message: "invalid Charles XML export",
    }
}

fn evidence_request(transaction: RawTransaction, route_ref: String) -> EvidenceRequest {
    let duration_millis = transaction.duration_millis.or_else(|| {
        transaction
            .end_millis
            .zip(transaction.start_millis)
            .and_then(|(end, start)| end.checked_sub(start))
    });
    let path_query = transaction
        .path
        .split_once('?')
        .map_or("", |(_, query)| query);
    let query = if transaction.query.is_empty() {
        path_query
    } else {
        &transaction.query
    };
    EvidenceRequest {
        method: safe_method(&transaction.method),
        route_ref,
        query_parameter_count: query.split('&').filter(|part| !part.is_empty()).count(),
        status: transaction.status,
        duration_millis,
        request_size_bytes: transaction.request_size_bytes,
        response_size_bytes: transaction.response_size_bytes,
    }
}

fn safe_method(method: &str) -> &'static str {
    match method {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        "CONNECT" => "CONNECT",
        "TRACE" => "TRACE",
        _ => "OTHER",
    }
}
