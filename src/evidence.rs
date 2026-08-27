use std::{
    collections::BTreeMap,
    fs::File,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

use quick_xml::{
    encoding::Decoder,
    events::{BytesStart, Event},
    Reader,
};
use rustix::fs::{fstat, open, openat, FileType, Mode, OFlags};
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

#[derive(Debug)]
struct CurrentTransaction {
    transaction: RawTransaction,
    request_seen: bool,
    response_seen: bool,
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
    analyze_with_after_open(evidence_root, xml_file, profile_name, source_host, || {})
}

fn analyze_with_after_open(
    evidence_root: &Path,
    xml_file: &Path,
    profile_name: &str,
    source_host: &str,
    after_open: impl FnOnce(),
) -> Result<serde_json::Value, EvidenceError> {
    let file = open_xml_file(evidence_root, xml_file)?;
    after_open();
    analyze_file(file, profile_name, source_host)
}

fn analyze_file(
    file: File,
    profile_name: &str,
    source_host: &str,
) -> Result<serde_json::Value, EvidenceError> {
    let xml = read_bounded_xml(file)?;
    let mut reader = Reader::from_reader(Cursor::new(xml));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut current = None;
    let mut summary = EvidenceSummary::default();
    let mut requests = Vec::new();
    let mut failures = Vec::new();
    let mut routes = BTreeMap::new();
    let mut root_open = false;
    let mut root_closed = false;
    let mut depth = 0_usize;

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
                if root_open || root_closed || depth != 0 {
                    return Err(malformed());
                }
                root_open = true;
                depth = 1;
            }
            Event::Empty(start) if start.name().as_ref() == b"charles-session" => {
                if root_open || root_closed || depth != 0 {
                    return Err(malformed());
                }
                root_closed = true;
            }
            Event::Start(start) if start.name().as_ref() == b"transaction" => {
                if !root_open || current.is_some() || depth != 1 {
                    return Err(malformed());
                }
                current = Some(CurrentTransaction {
                    transaction: transaction(&start, reader.decoder())?,
                    request_seen: false,
                    response_seen: false,
                });
                depth = 2;
            }
            Event::Empty(start) if start.name().as_ref() == b"transaction" => {
                return Err(malformed());
            }
            Event::Start(start) if start.name().as_ref() == b"request" => {
                let Some(current) = current.as_mut() else {
                    return Err(malformed());
                };
                if depth != 2 || current.request_seen || current.response_seen {
                    return Err(malformed());
                }
                current.request_seen = true;
                current.transaction.request_size_bytes = exchange_size(&start, reader.decoder())?;
                depth = 3;
            }
            Event::Empty(start) if start.name().as_ref() == b"request" => {
                let Some(current) = current.as_mut() else {
                    return Err(malformed());
                };
                if depth != 2 || current.request_seen || current.response_seen {
                    return Err(malformed());
                }
                current.request_seen = true;
                current.transaction.request_size_bytes = exchange_size(&start, reader.decoder())?;
            }
            Event::Start(start) if start.name().as_ref() == b"response" => {
                let Some(current) = current.as_mut() else {
                    return Err(malformed());
                };
                if depth != 2 || !current.request_seen || current.response_seen {
                    return Err(malformed());
                }
                current.response_seen = true;
                current.transaction.status = attribute(&start, b"status", reader.decoder())?
                    .and_then(|value| value.parse().ok());
                current.transaction.response_size_bytes = exchange_size(&start, reader.decoder())?;
                depth = 3;
            }
            Event::Empty(start) if start.name().as_ref() == b"response" => {
                let Some(current) = current.as_mut() else {
                    return Err(malformed());
                };
                if depth != 2 || !current.request_seen || current.response_seen {
                    return Err(malformed());
                }
                current.response_seen = true;
                current.transaction.status = attribute(&start, b"status", reader.decoder())?
                    .and_then(|value| value.parse().ok());
                current.transaction.response_size_bytes = exchange_size(&start, reader.decoder())?;
            }
            Event::End(end) if end.name().as_ref() == b"transaction" => {
                if depth != 2 {
                    return Err(malformed());
                }
                let Some(current) = current.take() else {
                    return Err(malformed());
                };
                let transaction = current.transaction;
                depth = 1;
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
                if !root_open || current.is_some() || depth != 1 {
                    return Err(malformed());
                }
                root_open = false;
                root_closed = true;
                depth = 0;
            }
            Event::Start(_) => {
                if !root_open || root_closed {
                    return Err(malformed());
                }
                depth = depth.checked_add(1).ok_or_else(malformed)?;
            }
            Event::Empty(_) => {
                if !root_open || root_closed {
                    return Err(malformed());
                }
            }
            Event::End(_) => {
                if !root_open || root_closed || depth <= 1 {
                    return Err(malformed());
                }
                depth -= 1;
            }
            Event::Eof => {
                if !root_closed || root_open || current.is_some() || depth != 0 {
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

fn read_bounded_xml(reader: impl Read) -> Result<Box<[u8]>, EvidenceError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_XML_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| read_failed())?;
    if bytes.len() as u64 > MAX_XML_BYTES {
        return Err(too_large());
    }
    Ok(bytes.into_boxed_slice())
}

fn open_xml_file(evidence_root: &Path, xml_file: &Path) -> Result<File, EvidenceError> {
    let evidence_root: PathBuf = std::path::absolute(evidence_root)
        .map_err(|_| evidence_root_unavailable())?
        .components()
        .collect();
    let relative = if xml_file.is_absolute() {
        xml_file
            .strip_prefix(&evidence_root)
            .map_err(|_| outside_root())?
    } else {
        xml_file
    };
    if !relative
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
    {
        return Err(EvidenceError {
            code: "session_file_not_xml",
            message: "the selected local file must use the .xml extension",
        });
    }
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(component) => components.push(component),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(outside_root());
            }
        }
    }
    let Some((file_name, parent_components)) = components.split_last() else {
        return Err(EvidenceError {
            code: "session_file_not_xml",
            message: "the selected local path must be an XML file",
        });
    };

    let directory_flags =
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let mut directory = open(&evidence_root, directory_flags, Mode::empty())
        .map_err(|_| evidence_root_unavailable())?;
    for component in parent_components {
        directory = openat(&directory, *component, directory_flags, Mode::empty())
            .map_err(component_open_error)?;
    }
    let file_descriptor = openat(
        &directory,
        *file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
    )
    .map_err(component_open_error)?;
    let stat = fstat(&file_descriptor).map_err(|_| read_failed())?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(EvidenceError {
            code: "session_file_not_xml",
            message: "the selected local path must be an XML file",
        });
    }
    if u64::try_from(stat.st_size).unwrap_or(u64::MAX) > MAX_XML_BYTES {
        return Err(too_large());
    }
    Ok(File::from(file_descriptor))
}

fn component_open_error(error: rustix::io::Errno) -> EvidenceError {
    if matches!(error, rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) {
        outside_root()
    } else {
        read_failed()
    }
}

fn read_failed() -> EvidenceError {
    EvidenceError {
        code: "session_read_failed",
        message: "unable to read the selected local XML file",
    }
}

fn too_large() -> EvidenceError {
    EvidenceError {
        code: "session_file_too_large",
        message: "the selected XML file exceeds the 10 MiB limit",
    }
}

fn evidence_root_unavailable() -> EvidenceError {
    EvidenceError {
        code: "evidence_root_unavailable",
        message: "the configured evidence root is not an accessible directory",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_retained_file_after_its_parent_path_is_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let evidence_root = directory.path().join("evidence");
        let selected_parent = evidence_root.join("selected");
        let retained_parent = evidence_root.join("retained");
        std::fs::create_dir_all(&selected_parent).unwrap();
        std::fs::write(
            selected_parent.join("session.xml"),
            r#"<charles-session><transaction method="GET" host="app.example.com" path="/original" query=""><request headers="0" body="0"/><response status="200" headers="0" body="0"/></transaction></charles-session>"#,
        )
        .unwrap();

        let evidence = analyze_with_after_open(
            &evidence_root,
            Path::new("selected/session.xml"),
            "demo",
            "app.example.com",
            || {
                std::fs::rename(&selected_parent, &retained_parent).unwrap();
                std::fs::create_dir(&selected_parent).unwrap();
                std::fs::write(
                    selected_parent.join("session.xml"),
                    r#"<charles-session><transaction method="GET" host="app.example.com" path="/replacement" query=""><request headers="0" body="0"/><response status="500" headers="0" body="0"/></transaction></charles-session>"#,
                )
                .unwrap();
            },
        )
        .unwrap();

        assert_eq!(evidence["summary"]["transactionCount"], 1);
        assert_eq!(evidence["summary"]["failureCount"], 0);
        assert_eq!(evidence["requests"][0]["status"], 200);
    }

    #[test]
    fn bounded_reader_rejects_content_that_grows_beyond_the_file_limit() {
        let input = vec![b'x'; MAX_XML_BYTES as usize + 1];

        let error = read_bounded_xml(std::io::Cursor::new(input)).unwrap_err();

        assert_eq!(error.code, "session_file_too_large");
        assert_eq!(
            error.message,
            "the selected XML file exceeds the 10 MiB limit"
        );
    }
}
