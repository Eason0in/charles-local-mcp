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

#[cfg(unix)]
#[test]
fn rejects_symlinked_parent_even_when_it_points_inside_the_evidence_root() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let evidence_root = directory.path().join("evidence");
    let real_parent = evidence_root.join("real-parent");
    std::fs::create_dir_all(&real_parent).unwrap();
    std::fs::write(
        real_parent.join("session.xml"),
        "<charles-session></charles-session>",
    )
    .unwrap();
    symlink(&real_parent, evidence_root.join("linked-parent")).unwrap();

    let (status, response, stderr) = run_evidence_with_root(
        std::path::Path::new("linked-parent/session.xml"),
        &evidence_root,
    );

    assert!(!status.success(), "stderr: {stderr}; response: {response}");
    assert_eq!(response["error"]["code"], "session_file_outside_root");
}

#[cfg(unix)]
#[test]
fn rejects_a_symlinked_evidence_root() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let real_root = directory.path().join("real-root");
    std::fs::create_dir(&real_root).unwrap();
    std::fs::write(
        real_root.join("session.xml"),
        "<charles-session></charles-session>",
    )
    .unwrap();
    let linked_root = directory.path().join("linked-root");
    symlink(&real_root, &linked_root).unwrap();
    let linked_root_with_slash = std::path::PathBuf::from(format!("{}/", linked_root.display()));
    let linked_root_with_dot = linked_root.join(".");

    for root in [
        linked_root.as_path(),
        linked_root_with_slash.as_path(),
        linked_root_with_dot.as_path(),
    ] {
        let (status, response, stderr) =
            run_evidence_with_root(std::path::Path::new("session.xml"), root);

        assert!(
            !status.success(),
            "root: {}; stderr: {stderr}; response: {response}",
            root.display()
        );
        assert_eq!(response["error"]["code"], "evidence_root_unavailable");
    }
}

#[cfg(unix)]
#[test]
fn rejects_a_final_symlink_even_with_a_trailing_slash() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let evidence_root = directory.path().join("evidence");
    std::fs::create_dir(&evidence_root).unwrap();
    let target = evidence_root.join("target.xml");
    std::fs::write(&target, "<charles-session></charles-session>").unwrap();
    symlink(&target, evidence_root.join("symlink.xml")).unwrap();

    let (status, response, stderr) =
        run_evidence_with_root(std::path::Path::new("symlink.xml/"), &evidence_root);

    assert!(!status.success(), "stderr: {stderr}; response: {response}");
    assert_eq!(response["error"]["code"], "session_file_outside_root");
}

#[test]
fn accepts_an_absolute_xml_path_with_a_relative_evidence_root() {
    let directory = tempfile::tempdir().unwrap();
    let child_working_directory = directory.path().canonicalize().unwrap();
    let evidence_root = directory.path().join("evidence");
    std::fs::create_dir(&evidence_root).unwrap();
    let xml = child_working_directory.join("evidence/session.xml");
    std::fs::write(&xml, "<charles-session></charles-session>").unwrap();
    let profiles = profiles_file(&directory);
    let output = Command::new(env!("CARGO_BIN_EXE_charles-local-mcp"))
        .current_dir(&child_working_directory)
        .args([
            "--state-dir",
            directory.path().join("state").to_str().unwrap(),
            "--evidence-root",
            "evidence",
            "--profiles-file",
            profiles.to_str().unwrap(),
            "session",
            "evidence",
            "--profile",
            "demo",
            "--xml-file",
            xml.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert!(output.status.success(), "response: {response}");
    assert_eq!(response["status"], "ready");
}

#[cfg(unix)]
#[test]
fn rejects_a_fifo_without_blocking() {
    use std::time::{Duration, Instant};

    let directory = tempfile::tempdir().unwrap();
    let evidence_root = directory.path().join("evidence");
    std::fs::create_dir(&evidence_root).unwrap();
    let fifo = evidence_root.join("session.xml");
    let mkfifo = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(mkfifo.success());
    let profiles = profiles_file(&directory);
    let mut child = Command::new(env!("CARGO_BIN_EXE_charles-local-mcp"))
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
            "session.xml",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
        let _ = child.wait();
        panic!("session evidence blocked while opening a FIFO");
    }
    let output = child.wait_with_output().unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert!(!output.status.success());
    assert_eq!(response["error"]["code"], "session_file_not_xml");
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
fn profile_failures_use_fixed_errors_without_echoing_paths_config_or_input() {
    let directory = tempfile::tempdir().unwrap();
    let evidence_root = directory.path().join("evidence");
    std::fs::create_dir(&evidence_root).unwrap();
    std::fs::write(
        evidence_root.join("session.xml"),
        "<charles-session></charles-session>",
    )
    .unwrap();
    let sensitive_path_token = "sensitive-profiles-location-token";
    let missing_profiles = directory
        .path()
        .join(sensitive_path_token)
        .join("profiles.toml");
    let invalid_profiles = directory.path().join("invalid-profiles.toml");
    let sensitive_source_host = "private-source-host-token/invalid";
    std::fs::write(
        &invalid_profiles,
        format!("schemaVersion = 1\n[profiles.demo]\nsourceHost = \"{sensitive_source_host}\"\n"),
    )
    .unwrap();
    let valid_profiles = profiles_file(&directory);
    let oversized_profile = format!("private-profile-token-{}", "x".repeat(80));
    let cases = [
        (missing_profiles, "demo".to_owned()),
        (invalid_profiles, "demo".to_owned()),
        (valid_profiles.clone(), "unknown-profile-token".to_owned()),
        (valid_profiles.clone(), oversized_profile.clone()),
        (valid_profiles, "invalid/profile-token".to_owned()),
    ];

    for (index, (profiles, requested_profile)) in cases.into_iter().enumerate() {
        let service = Service::with_evidence_root(
            directory.path().join(format!("state-{index}")),
            Some(profiles),
            Some(evidence_root.clone()),
        )
        .unwrap();
        let response = service.session_evidence(SessionEvidenceRequest {
            profile: requested_profile,
            xml_file: "session.xml".into(),
        });

        let error = response.error.as_ref().unwrap();
        assert_eq!(error.code, "invalid_profile");
        assert_eq!(
            error.message,
            "the selected evidence profile is unavailable"
        );
        let serialized = serde_json::to_string(&response).unwrap();
        for secret in [
            sensitive_path_token,
            sensitive_source_host,
            "unknown-profile-token",
            oversized_profile.as_str(),
            "invalid/profile-token",
        ] {
            assert!(
                !serialized.contains(secret),
                "leaked profile detail: {secret}"
            );
        }
    }
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

#[test]
fn requires_single_ordered_direct_request_and_response_elements() {
    let directory = tempfile::tempdir().unwrap();
    let legitimate = write_xml(
        &directory,
        "legitimate-subtrees.xml",
        r#"<charles-session><transaction method="POST" host="app.example.com" path="/safe" query=""><request headers="2" body="3"><headers><header><name>secret-name</name><value>secret-value</value></header></headers><body>secret-body</body></request><response status="200" headers="5" body="8"><headers><header><name>secret-response-name</name><value>secret-response-value</value></header></headers><body>secret-response-body</body></response></transaction></charles-session>"#,
    );
    let (status, response, stderr) = run_evidence(&legitimate);
    assert!(status.success(), "stderr: {stderr}; response: {response}");
    assert_eq!(response["data"]["requests"][0]["requestSizeBytes"], 5);
    assert_eq!(response["data"]["requests"][0]["responseSizeBytes"], 13);
    let serialized = serde_json::to_string(&response).unwrap();
    for secret in [
        "secret-name",
        "secret-value",
        "secret-body",
        "secret-response-name",
        "secret-response-value",
        "secret-response-body",
    ] {
        assert!(!serialized.contains(secret));
    }

    let invalid_documents = [
        (
            "nested-request.xml",
            r#"<charles-session><transaction method="GET" host="app.example.com" path="/safe" query=""><wrapper><request headers="0" body="0"/></wrapper><response status="200" headers="0" body="0"/></transaction></charles-session>"#,
        ),
        (
            "repeated-request.xml",
            r#"<charles-session><transaction method="GET" host="app.example.com" path="/safe" query=""><request headers="0" body="0"/><request headers="0" body="0"/><response status="200" headers="0" body="0"/></transaction></charles-session>"#,
        ),
        (
            "response-before-request.xml",
            r#"<charles-session><transaction method="GET" host="app.example.com" path="/safe" query=""><response status="200" headers="0" body="0"/><request headers="0" body="0"/></transaction></charles-session>"#,
        ),
        (
            "repeated-response.xml",
            r#"<charles-session><transaction method="GET" host="app.example.com" path="/safe" query=""><request headers="0" body="0"/><response status="200" headers="0" body="0"/><response status="500" headers="0" body="0"/></transaction></charles-session>"#,
        ),
        (
            "request-inside-response.xml",
            r#"<charles-session><transaction method="GET" host="app.example.com" path="/safe" query=""><request headers="0" body="0"/><response status="200" headers="0" body="0"><request headers="9" body="9"/></response></transaction></charles-session>"#,
        ),
    ];
    for (name, document) in invalid_documents {
        let xml = write_xml(&directory, name, document);
        let (status, response, stderr) = run_evidence(&xml);
        assert!(!status.success(), "stderr: {stderr}; response: {response}");
        assert_eq!(response["error"]["code"], "session_xml_malformed");
    }
}
