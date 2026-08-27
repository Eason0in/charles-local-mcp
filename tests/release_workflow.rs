#[test]
fn release_workflow_keeps_non_apple_publication_gates() {
    let workflow = include_str!("../.github/workflows/release.yml");

    assert!(!workflow.contains("APPLE_"));
    assert!(!workflow.contains("codesign"));
    assert!(!workflow.contains("notarytool"));
    assert!(workflow.contains("aarch64-apple-darwin,x86_64-apple-darwin"));
    assert!(workflow.contains("lipo -create"));
    assert!(workflow.contains("https://github.com/Eason0in.gpg"));
    assert!(workflow.contains("0EB5965CB6DF87E3F7FFEA2758C03387F54FDA7A"));
    assert!(workflow.contains("gpg --batch --import"));
    assert!(workflow.contains("test \"$actual_fingerprint\" = \"$expected_fingerprint\""));
    assert!(workflow.contains("git verify-tag"));
    let key_download = workflow.find("https://github.com/Eason0in.gpg").unwrap();
    let key_import = workflow.find("gpg --batch --import").unwrap();
    let fingerprint_check = workflow
        .find("test \"$actual_fingerprint\" = \"$expected_fingerprint\"")
        .unwrap();
    let tag_verification = workflow.find("git verify-tag").unwrap();
    assert!(key_download < key_import);
    assert!(key_import < fingerprint_check);
    assert!(fingerprint_check < tag_verification);
    assert!(workflow.contains("@anthropic-ai/mcpb@2.1.2 pack"));
    assert!(workflow.contains("RELEASE-NOTICE.txt"));
    assert!(workflow.contains("cargo publish --locked --token"));
    assert!(workflow.contains("gh release create"));
    assert!(workflow.contains("actions/attest-build-provenance@v3"));
    assert!(workflow.contains("mcp-publisher publish dist/server.json"));
}

#[test]
fn readme_discloses_that_native_assets_are_not_notarized() {
    let readme = include_str!("../README.md");

    assert!(readme.contains("not signed or notarized by Apple"));
    assert!(readme.contains("security warning"));
    assert!(readme.contains("session evidence"));
    assert!(readme.contains("headers, cookies, credentials, or bodies"));
    assert!(readme.contains("DOCTYPE"));
    assert!(readme.contains("--evidence-root"));
    assert!(readme.contains("opaque route references"));
    assert!(readme.contains("never returns raw hosts, paths, query names, query values"));
}

#[test]
fn registry_metadata_uses_the_oidc_publisher_namespace() {
    let metadata: serde_json::Value =
        serde_json::from_str(include_str!("../server.template.json")).unwrap();

    assert_eq!(
        metadata["name"], "io.github.Eason0in/charles-local-mcp",
        "the Registry server identifier must match the GitHub OIDC publisher namespace"
    );
}

#[test]
fn mcpb_build_script_uses_the_current_release_version_by_default() {
    let script = include_str!("../scripts/build-mcpb.sh");

    assert!(script.contains("charles-local-mcp-0.1.3-"));
    assert!(!script.contains("charles-local-mcp-0.1.2-"));
}

#[test]
fn release_metadata_is_consistently_versioned_and_lists_session_evidence() {
    let cargo: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let lock = include_str!("../Cargo.lock");
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../mcpb/manifest.json")).unwrap();
    let server: serde_json::Value =
        serde_json::from_str(include_str!("../server.template.json")).unwrap();

    assert_eq!(cargo["package"]["version"].as_str(), Some("0.1.3"));
    assert!(lock.contains("name = \"charles-local-mcp\"\nversion = \"0.1.3\""));
    assert_eq!(manifest["version"], "0.1.3");
    assert_eq!(server["version"], "0.1.3");
    assert_eq!(server["packages"][0]["version"], "0.1.3");
    assert!(server["packages"][0]["identifier"]
        .as_str()
        .unwrap()
        .contains("/v0.1.3/charles-local-mcp-0.1.3-"));
    assert!(manifest["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "session_evidence"));
    assert!(manifest["server"]["mcp_config"]["args"]
        .as_array()
        .unwrap()
        .iter()
        .any(|argument| argument == "--evidence-root"));
    assert_eq!(
        manifest["user_config"]["evidence_root"]["type"],
        "directory"
    );
    assert_eq!(manifest["user_config"]["evidence_root"]["required"], true);
}
