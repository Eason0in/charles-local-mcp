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
    let requests = concat!(
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"profiles_validate\",\"arguments\":{}}}\n"
    );
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
    let called = messages.iter().find(|message| message["id"] == 3).unwrap();
    assert_eq!(
        called["result"]["structuredContent"]["contractVersion"],
        "charles-local/v1"
    );
    assert_eq!(called["result"]["structuredContent"]["status"], "ready");
}
