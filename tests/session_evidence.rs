use std::process::Command;

use charles_local_mcp::{Service, SessionEvidenceRequest};
use serde_json::Value;

fn profiles_file(directory: &tempfile::TempDir) -> std::path::PathBuf {
    let path = directory.path().join("profiles.toml");
    std::fs::write(
        &path,
        r#"schemaVersion = 1

[profiles.demo]
sourceHost = "app.example.com"
destinationUrl = "http://127.0.0.1:8080"
"#,
    )
    .unwrap();
    path
}

fn run_evidence(xml_file: &std::path::Path) -> (std::process::ExitStatus, Value, String) {
    run_evidence_with_root(xml_file, xml_file.parent().unwrap())
}

fn run_evidence_with_root(
    xml_file: &std::path::Path,
    evidence_root: &std::path::Path,
) -> (std::process::ExitStatus, Value, String) {
    let directory = tempfile::tempdir().unwrap();
    let profiles = profiles_file(&directory);
    let output = Command::new(env!("CARGO_BIN_EXE_charles-local-mcp"))
        .args([
            "--state-dir",
            directory.path().join("state").to_str().unwrap(),
            "--evidence-root",
            evidence_root.to_str().unwrap(),
            "--profiles-file",
            profiles.to_str().unwrap(),
            "session",
            "evidence",
            "--profile",
            "demo",
            "--xml-file",
            xml_file.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
    (
        output.status,
        value,
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn write_xml(directory: &tempfile::TempDir, name: &str, contents: &str) -> std::path::PathBuf {
    let path = directory.path().join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn transaction(index: usize, status: u16) -> String {
    format!(
        r#"<transaction method="GET" host="app.example.com" path="/api/items/{index}" query="token=secret-{index}" startTimeMillis="{index}" endTimeMillis="{}"><request headers="1" body="0"/><response status="{status}" headers="1" body="1"/></transaction>"#,
        index + 1
    )
}

fn files_below(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_below(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn summarizes_only_the_profile_host_and_never_emits_sensitive_session_content() {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/charles-session.xml");

    let (status, response, stderr) = run_evidence(&fixture);

    assert!(status.success(), "stderr: {stderr}; response: {response}");
    assert_eq!(response["operation"], "session.evidence");
    assert_eq!(response["status"], "ready");
    let evidence = &response["data"];
    assert_eq!(evidence["schemaVersion"], "charles-session-evidence/v1");
    assert_eq!(evidence["source"]["format"], "charles_xml_export");
    assert_eq!(evidence["source"]["localOnly"], true);
    assert_eq!(evidence["profile"]["name"], "demo");
    assert_eq!(evidence["profile"]["hostScope"], "exact");
    assert_eq!(evidence["summary"]["transactionCount"], 4);
    assert_eq!(evidence["summary"]["includedCount"], 3);
    assert_eq!(evidence["summary"]["excludedHostCount"], 1);
    assert_eq!(evidence["summary"]["failureCount"], 2);
    assert_eq!(evidence["summary"]["status4xxCount"], 1);
    assert_eq!(evidence["summary"]["status5xxCount"], 1);

    let requests = evidence["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["method"], "GET");
    assert_eq!(requests[0]["routeRef"], "route-001");
    assert_eq!(requests[0]["queryParameterCount"], 2);
    assert!(requests[0].get("host").is_none());
    assert!(requests[0].get("path").is_none());
    assert_eq!(requests[0]["status"], 200);
    assert_eq!(requests[0]["durationMillis"], 42);
    assert_eq!(requests[0]["requestSizeBytes"], 120);
    assert_eq!(requests[0]["responseSizeBytes"], 100);
    assert_eq!(requests[1]["routeRef"], "route-002");
    assert_eq!(requests[2]["routeRef"], "route-003");
    assert_eq!(requests[2]["durationMillis"], 750);

    let failures = evidence["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0]["status"], 409);
    assert_eq!(failures[0]["routeRef"], "route-002");
    assert_eq!(failures[1]["status"], 503);

    let serialized = serde_json::to_string(&response).unwrap();
    for secret in [
        "Authorization",
        "Cookie",
        "Set-Cookie",
        "app.example.com",
        "/api/health",
        "token",
        "mode",
        "top-secret-query",
        "full",
        "retry-secret",
        "super-secret-token",
        "session-cookie-secret",
        "response-cookie-secret",
        "cmVxdWVzdC1ib2R5LXNlY3JldA==",
        "cmVzcG9uc2UtYm9keS1zZWNyZXQ=",
        "telemetry.example.net",
        "excluded-host-secret",
        "excluded-secret",
        "ZXhjbHVkZWQtYm9keS1zZWNyZXQ=",
    ] {
        assert!(
            !serialized.contains(secret),
            "leaked sensitive value: {secret}"
        );
    }
    assert!(!serialized.contains(fixture.to_str().unwrap()));
}

#[test]
fn counts_query_parameters_without_emitting_names_values_or_paths() {
    let directory = tempfile::tempdir().unwrap();
    let document = r#"<charles-session><transaction method="GET" host="app.example.com" path="/api" query="bare-query-secret&amp;safe=value-secret"><request headers="0" body="0"/><response status="200" headers="0" body="0"/></transaction></charles-session>"#;
    let xml = write_xml(&directory, "bare-query.xml", document);

    let (status, response, stderr) = run_evidence(&xml);

    assert!(status.success(), "stderr: {stderr}; response: {response}");
    assert_eq!(response["data"]["requests"][0]["routeRef"], "route-001");
    assert_eq!(response["data"]["requests"][0]["queryParameterCount"], 2);
    let serialized = serde_json::to_string(&response).unwrap();
    assert!(!serialized.contains("/api"));
    assert!(!serialized.contains("bare-query-secret"));
    assert!(!serialized.contains("safe"));
    assert!(!serialized.contains("value-secret"));
}

#[test]
fn assigns_opaque_route_refs_and_whitelists_methods_without_emitting_packet_strings() {
    let directory = tempfile::tempdir().unwrap();
    let document = r#"<charles-session>
<transaction method="GET" host="app.example.com" path="/users/alice/profile" query="query-key-secret=value-secret"><request headers="0" body="0"/><response status="200" headers="0" body="0"/></transaction>
<transaction method="GET" host="app.example.com" path="/users/王小明/profile" query=""><request headers="0" body="0"/><response status="200" headers="0" body="0"/></transaction>
<transaction method="POST" host="app.example.com" path="/orders/A7K2" query=""><request headers="0" body="0"/><response status="200" headers="0" body="0"/></transaction>
<transaction method="CUSTOM_SECRET" host="app.example.com" path="/users/alice/profile" query="query-key-secret=other-value"><request headers="0" body="0"/><response status="500" headers="0" body="0"/></transaction>
<transaction method="HEAD" host="app.example.com" path="/health" query=""><request headers="0" body="0"/><response status="200" headers="0" body="0"/></transaction>
</charles-session>"#;
    let xml = write_xml(&directory, "sensitive-paths.xml", document);

    let (status, response, stderr) = run_evidence(&xml);

    assert!(status.success(), "stderr: {stderr}; response: {response}");
    let requests = response["data"]["requests"].as_array().unwrap();
    assert_eq!(requests[0]["routeRef"], "route-001");
    assert_eq!(requests[0]["queryParameterCount"], 1);
    assert_eq!(requests[1]["routeRef"], "route-002");
    assert_eq!(requests[2]["routeRef"], "route-003");
    assert_eq!(requests[3]["routeRef"], "route-001");
    assert_eq!(requests[3]["method"], "OTHER");
    assert_eq!(requests[4]["routeRef"], "route-004");
    assert_eq!(requests[4]["method"], "HEAD");
    let serialized = serde_json::to_string(&response).unwrap();
    for secret in [
        "/users/alice/profile",
        "alice",
        "王小明",
        "/orders/A7K2",
        "A7K2",
        "/health",
        "query-key-secret",
        "value-secret",
        "other-value",
        "CUSTOM_SECRET",
        "app.example.com",
    ] {
        assert!(
            !serialized.contains(secret),
            "leaked path segment: {secret}"
        );
    }
}

#[cfg(unix)]
#[test]
fn confines_selected_files_to_the_canonical_evidence_root() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let allowed = directory.path().join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let document = "<charles-session></charles-session>";
    let inside = allowed.join("inside.xml");
    std::fs::write(&inside, document).unwrap();
    let outside = directory.path().join("outside.xml");
    std::fs::write(&outside, document).unwrap();
    let symlink_escape = allowed.join("symlink.xml");
    symlink(&outside, &symlink_escape).unwrap();

    let (inside_status, inside_response, inside_stderr) =
        run_evidence_with_root(std::path::Path::new("inside.xml"), &allowed);
    assert!(
        inside_status.success(),
        "stderr: {inside_stderr}; response: {inside_response}"
    );

    for requested in [
        std::path::Path::new("../outside.xml"),
        outside.as_path(),
        symlink_escape.as_path(),
    ] {
        let (status, response, stderr) = run_evidence_with_root(requested, &allowed);
        assert!(!status.success(), "stderr: {stderr}; response: {response}");
        assert_eq!(response["error"]["code"], "session_file_outside_root");
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains(outside.to_str().unwrap()));
    }
}

#[test]
fn evidence_analysis_is_read_only_and_writes_no_session_or_ledger_files() {
    let directory = tempfile::tempdir().unwrap();
    let evidence_root = directory.path().join("evidence");
    std::fs::create_dir(&evidence_root).unwrap();
    let xml = evidence_root.join("session.xml");
    let fixture = include_bytes!("../fixtures/charles-session.xml");
    std::fs::write(&xml, fixture).unwrap();
    let profiles = profiles_file(&directory);
    let profiles_before = std::fs::read(&profiles).unwrap();
    let state_dir = directory.path().join("state");
    let service = Service::with_evidence_root(
        state_dir.clone(),
        Some(profiles.clone()),
        Some(evidence_root),
    )
    .unwrap();

    let response = service.session_evidence(SessionEvidenceRequest {
        profile: "demo".into(),
        xml_file: "session.xml".into(),
    });

    assert_eq!(
        response.status,
        charles_local_mcp::model::ResponseStatus::Ready
    );
    assert_eq!(std::fs::read(xml).unwrap(), fixture);
    assert_eq!(std::fs::read(profiles).unwrap(), profiles_before);
    assert!(files_below(&state_dir).is_empty());
}

#[test]
fn rejects_doctype_and_entity_references_without_echoing_them() {
    let directory = tempfile::tempdir().unwrap();
    let unsafe_documents = [
        r#"<?xml version="1.0"?>
<!DOCTYPE charles-session [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
<charles-session><transaction method="GET" host="app.example.com" path="/&xxe;" query=""><request headers="0" body="0"/><response status="200" headers="0" body="0"/></transaction></charles-session>"#,
        r#"<?xml version="1.0"?>
<charles-session><transaction method="GET" host="app.example.com" path="/safe" query=""><request headers="0" body="0"><body>&undeclared-secret;</body></request><response status="200" headers="0" body="0"/></transaction></charles-session>"#,
    ];

    for (index, document) in unsafe_documents.iter().enumerate() {
        let path = write_xml(&directory, &format!("unsafe-{index}.xml"), document);
        let (status, response, stderr) = run_evidence(&path);
        assert!(!status.success(), "stderr: {stderr}; response: {response}");
        assert_eq!(response["error"]["code"], "session_xml_unsafe");
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains("/etc/passwd"));
        assert!(!serialized.contains("undeclared-secret"));
    }
}

#[test]
fn rejects_non_xml_malformed_and_oversized_inputs_with_stable_codes() {
    let directory = tempfile::tempdir().unwrap();
    let not_xml = write_xml(
        &directory,
        "session.txt",
        "<charles-session></charles-session>",
    );
    let malformed = write_xml(
        &directory,
        "malformed.xml",
        "<charles-session><transaction></charles-session>",
    );
    let oversized = directory.path().join("oversized.xml");
    let file = std::fs::File::create(&oversized).unwrap();
    file.set_len(10 * 1024 * 1024 + 1).unwrap();

    for (path, expected_code) in [
        (not_xml, "session_file_not_xml"),
        (malformed, "session_xml_malformed"),
        (oversized, "session_file_too_large"),
    ] {
        let (status, response, stderr) = run_evidence(&path);
        assert!(!status.success(), "stderr: {stderr}; response: {response}");
        assert_eq!(response["error"]["code"], expected_code);
    }
}

#[test]
fn bounds_evidence_output_and_rejects_excessive_transaction_counts() {
    let directory = tempfile::tempdir().unwrap();
    let bounded_xml = format!(
        "<charles-session>{}</charles-session>",
        (0..101)
            .map(|index| transaction(index, if index >= 50 { 500 } else { 200 }))
            .collect::<String>()
    );
    let bounded = write_xml(&directory, "bounded.xml", &bounded_xml);

    let (status, response, stderr) = run_evidence(&bounded);

    assert!(status.success(), "stderr: {stderr}; response: {response}");
    assert_eq!(response["data"]["requests"].as_array().unwrap().len(), 100);
    assert_eq!(response["data"]["failures"].as_array().unwrap().len(), 50);
    assert_eq!(response["data"]["summary"]["transactionCount"], 101);
    assert_eq!(response["data"]["summary"]["includedCount"], 101);
    assert_eq!(response["data"]["summary"]["failureCount"], 51);
    assert_eq!(response["data"]["summary"]["omittedRequestCount"], 1);
    assert_eq!(response["data"]["summary"]["omittedFailureCount"], 1);

    let excessive_xml = format!(
        "<charles-session>{}</charles-session>",
        (0..1001)
            .map(|index| transaction(index, 200))
            .collect::<String>()
    );
    let excessive = write_xml(&directory, "excessive.xml", &excessive_xml);
    let (status, response, stderr) = run_evidence(&excessive);
    assert!(!status.success(), "stderr: {stderr}; response: {response}");
    assert_eq!(response["error"]["code"], "session_item_limit_exceeded");
}

#[test]
fn rejects_wrong_roots_missing_required_fields_and_unbounded_output_fields() {
    let directory = tempfile::tempdir().unwrap();
    let long_path = format!("/{}", "a".repeat(2048));
    let too_many_query_parameters = (0..51)
        .map(|index| format!("field{index}=secret{index}"))
        .collect::<Vec<_>>()
        .join("&amp;");
    let cases = [
        (
            "wrong-root.xml",
            "<not-charles-session/>",
            "session_xml_malformed",
        ),
        (
            "missing-fields.xml",
            "<charles-session><transaction host=\"app.example.com\" path=\"/api\"><request headers=\"0\" body=\"0\"/><response status=\"200\" headers=\"0\" body=\"0\"/></transaction></charles-session>",
            "session_xml_malformed",
        ),
    ];
    for (name, document, expected_code) in cases {
        let path = write_xml(&directory, name, document);
        let (status, response, stderr) = run_evidence(&path);
        assert!(!status.success(), "stderr: {stderr}; response: {response}");
        assert_eq!(response["error"]["code"], expected_code);
    }

    for (name, path, query) in [
        ("long-path.xml", long_path.as_str(), ""),
        (
            "many-query-fields.xml",
            "/api",
            too_many_query_parameters.as_str(),
        ),
    ] {
        let document = format!(
            "<charles-session><transaction method=\"GET\" host=\"app.example.com\" path=\"{path}\" query=\"{query}\"><request headers=\"0\" body=\"0\"/><response status=\"200\" headers=\"0\" body=\"0\"/></transaction></charles-session>"
        );
        let xml = write_xml(&directory, name, &document);
        let (status, response, stderr) = run_evidence(&xml);
        assert!(!status.success(), "stderr: {stderr}; response: {response}");
        assert_eq!(response["error"]["code"], "session_field_limit_exceeded");
    }
}
