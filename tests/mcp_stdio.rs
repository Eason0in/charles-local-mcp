use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

#[test]
fn initialize_list_and_call_return_structured_contract() {
    let directory = tempfile::tempdir().unwrap();
    let profiles = directory.path().join("profiles.toml");
    std::fs::write(&profiles, "schemaVersion = 1\n[profiles.demo]\nsourceHost = \"app.example.com\"\ndestinationUrl = \"http://127.0.0.1:8080\"\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_charles-local-mcp"))
        .args([
            "--state-dir",
            directory.path().join("state").to_str().unwrap(),
            "--evidence-root",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .to_str()
                .unwrap(),
            "--profiles-file",
            profiles.to_str().unwrap(),
            "serve",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let initialize =
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n";
    let evidence_file =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/charles-session.xml");
    let requests = [
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "profiles_validate", "arguments": {}}
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "session_evidence",
                "arguments": {
                    "profile": "demo",
                    "xmlFile": evidence_file,
                }
            }
        }),
    ]
    .into_iter()
    .map(|request| request.to_string())
    .collect::<Vec<_>>()
    .join("\n")
        + "\n";
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(initialize.as_bytes()).unwrap();
    stdin.flush().unwrap();
    thread::sleep(Duration::from_millis(250));
    stdin.write_all(requests.as_bytes()).unwrap();
    stdin.flush().unwrap();
    thread::sleep(Duration::from_secs(1));
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let messages: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(messages
        .iter()
        .any(|message| message["id"] == 1 && message["result"]["serverInfo"].is_object()));
    let listed = messages
        .iter()
        .find(|message| message["id"] == 2)
        .unwrap_or_else(|| panic!("missing tools/list response: {messages:?}"));
    let tools = listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("invalid tools/list response: {listed:?}"));
    assert!(tools.iter().any(|tool| tool["name"] == "setup_plan"));
    let evidence_tool = tools
        .iter()
        .find(|tool| tool["name"] == "session_evidence")
        .expect("session_evidence tool");
    assert!(evidence_tool["description"]
        .as_str()
        .unwrap()
        .contains("configured evidence root"));
    assert!(evidence_tool["description"]
        .as_str()
        .unwrap()
        .contains("opaque route references"));
    assert!(
        evidence_tool["inputSchema"]["properties"]["xmlFile"]["description"]
            .as_str()
            .unwrap()
            .contains("configured evidence root")
    );
    let called = messages.iter().find(|message| message["id"] == 3).unwrap();
    assert_eq!(
        called["result"]["structuredContent"]["contractVersion"],
        "charles-local/v1"
    );
    assert_eq!(called["result"]["structuredContent"]["status"], "ready");
    let evidence = messages.iter().find(|message| message["id"] == 4).unwrap();
    assert_eq!(
        evidence["result"]["structuredContent"]["operation"],
        "session.evidence"
    );
    assert_eq!(
        evidence["result"]["structuredContent"]["data"]["summary"]["includedCount"],
        3
    );
    assert_eq!(
        evidence["result"]["structuredContent"]["data"]["profile"]["hostScope"],
        "exact"
    );
    assert!(evidence["result"]["structuredContent"]["data"]["profile"]
        .get("sourceHost")
        .is_none());
}
