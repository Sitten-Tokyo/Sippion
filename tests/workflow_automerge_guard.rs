//! Regression tests for the trusted-author auto-merge validation boundary.
//!
//! `.github/workflows/author-auto-merge.yml` is evaluated from the default branch and trusts
//! success signals (CI, smoke workflows, base/head binding artifacts) that are produced by
//! running workflow and script contents from the PR head. A PR that rewrites its own validators
//! could manufacture those signals, so PRs touching security-sensitive validation paths must be
//! excluded from the auto-merge path and require manual review. These tests pin that boundary:
//! they fail if the exclusion is removed or weakened.

use std::path::PathBuf;

fn automerge_workflow_source() -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/author-auto-merge.yml");
    std::fs::read_to_string(&path).expect("read trusted-author auto-merge workflow")
}

#[test]
fn workflow_touching_prs_are_excluded_from_trusted_author_auto_merge() {
    let source = automerge_workflow_source();
    assert!(
        source.contains(".github/workflows/*"),
        "auto-merge must match PR changes under .github/workflows/"
    );
    assert!(
        source.contains("zizmor.yml"),
        "auto-merge must treat the workflow-audit config as sensitive"
    );
    assert!(
        source.contains("deny.toml"),
        "auto-merge must treat the supply-chain policy config as sensitive"
    );
    assert!(
        source.contains("deny.toml"),
        "auto-merge must treat the cargo-deny policy as sensitive"
    );
    assert!(
        source.contains("scripts/check-bootstrap-pins.py"),
        "auto-merge must treat the bootstrap-pin verifier as sensitive"
    );
    assert!(
        source.contains("sensitive_validation_path=1"),
        "auto-merge must flag PRs touching validation paths"
    );
    assert!(
        source.contains("ready=false"),
        "flagged validation-path PRs must leave the merge unready for manual review"
    );
}

#[test]
fn auto_merge_still_binds_the_exact_tested_head() {
    let source = automerge_workflow_source();
    assert!(
        source.contains("--match-head-commit"),
        "auto-merge must only merge the exact CI-tested head commit"
    );
    assert!(
        source.contains("ci-binding-"),
        "auto-merge must keep validating the CI-recorded base/head binding artifact"
    );
}

#[test]
fn auto_merge_still_restricts_to_same_repo_trusted_author() {
    let source = automerge_workflow_source();
    assert!(
        source.contains(".head.repo.fork == false"),
        "auto-merge must keep refusing fork PRs"
    );
    assert!(
        source.contains(".user.login == \"Sitten-Tokyo\""),
        "auto-merge must keep restricting to the trusted author"
    );
}
