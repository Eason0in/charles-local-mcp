#[test]
fn release_workflow_keeps_non_apple_publication_gates() {
    let workflow = include_str!("../.github/workflows/release.yml");

    assert!(!workflow.contains("APPLE_"));
    assert!(!workflow.contains("codesign"));
    assert!(!workflow.contains("notarytool"));
    assert!(workflow.contains("aarch64-apple-darwin,x86_64-apple-darwin"));
    assert!(workflow.contains("lipo -create"));
    assert!(workflow.contains("git verify-tag"));
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
}
