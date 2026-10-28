//! A-008/F25: release assets live in a folder the source index excludes (`dist`), so the release
//! checker must read them as assets, not through the source snapshot's `read_text` (which would
//! mark the check unestablished and make release coherence permanently unavailable).

mod common;

use verdict_checkers::{Checker, real_registry};

#[test]
fn a_dist_asset_and_its_sidecar_are_checked_as_assets() {
    let project = common::project::project(&[("README.md", "# verdict\n")], false);
    std::fs::write(
        project.root().join("verdict.json"),
        r#"{"version":1,"release":{"assets":["dist/*.zip"]}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(project.root().join("dist")).unwrap();
    std::fs::write(project.root().join("dist/app.zip"), b"the packaged bytes").unwrap();
    // A sidecar that does not match the asset: this must be found, not skipped as unavailable.
    let wrong = verdict_core::sha256::hex(b"other bytes");
    std::fs::write(
        project.root().join("dist/app.zip.sha256"),
        format!("{wrong}  app.zip\n"),
    )
    .unwrap();

    let context = project.context(
        verdict_core::Lane::Release,
        std::time::Duration::from_secs(30),
    );
    let registry = real_registry();
    let release: &dyn Checker = registry
        .iter()
        .find(|c| c.name() == "release")
        .map(Box::as_ref)
        .expect("the release checker is registered");
    let findings = release
        .run(&context)
        .expect("reading dist must not make the release checker unavailable");
    assert!(
        findings
            .iter()
            .any(|f| f.id.contains("asset-hash-mismatch")),
        "the mismatched sidecar must be a finding: {findings:?}"
    );
    assert!(
        context.unestablished_reasons().is_empty(),
        "an asset read is not an unestablished source read: {:?}",
        context.unestablished_reasons()
    );
}
