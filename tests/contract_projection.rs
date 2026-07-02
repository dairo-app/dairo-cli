//! Contract test: the CLI's declared operations must be an honest subset of the
//! operations the live API actually publishes.
//!
//! The reference set (`contract/canonical-operations.json`) is **derived from
//! the canonical OpenAPI spec** (`dairo.openapi.json`) by
//! `scripts/gen-canonical-operations.py`, not hand-copied from this repo. That
//! is what keeps this test from being a tautology: the implemented projection
//! is compared against an independently-sourced, spec-derived list, so a stale
//! or over-claiming CLI surface fails the build.

use std::collections::BTreeSet;

#[test]
fn implemented_operations_are_an_honest_canonical_subset() {
    let canonical: serde_json::Value =
        serde_json::from_str(include_str!("../contract/canonical-operations.json")).unwrap();
    let implemented: serde_json::Value =
        serde_json::from_str(include_str!("../contract/implemented-operations.json")).unwrap();

    let canonical_ops = canonical["operations"].as_array().unwrap();
    let implemented_ops = implemented["operations"].as_array().unwrap();

    // Honesty guard: the CLI exposes a curated subset, never the entire API.
    // If these ever match, the projection is almost certainly a copy of the
    // canonical list rather than the real implemented surface.
    let canonical_count = canonical["operationCount"].as_u64().unwrap();
    let implemented_count = implemented["operationCount"].as_u64().unwrap();
    assert!(
        implemented_count < canonical_count,
        "the CLI contract must declare a strict subset of the live API \
         (implemented={implemented_count}, canonical={canonical_count}); \
         it must not claim full parity by copying the canonical projection"
    );
    assert_eq!(implemented["coverage"], "implemented-subset");

    // Every declared operationCount must match the actual array length so a
    // drifted hand-edit of one but not the other is caught.
    assert_eq!(
        implemented_count,
        implemented_ops.len() as u64,
        "implemented operationCount must match the operations array length"
    );
    assert_eq!(
        canonical_count,
        canonical_ops.len() as u64,
        "canonical operationCount must match the operations array length"
    );

    // Every implemented operation must exist in the canonical (spec-derived)
    // set, matched on method + path + operationId.
    let canonical_keys: BTreeSet<_> = canonical_ops.iter().map(operation_key).collect();
    for op in implemented_ops {
        assert!(
            canonical_keys.contains(&operation_key(op)),
            "implemented operation is absent from the canonical spec-derived projection: {op:?}"
        );
    }

    // Spot-check that the operations backed by bespoke CLI subcommands are still
    // declared (these were historically the easiest to drop on a refactor).
    for required in [
        "getInboxSchema",
        "setInboxSchema",
        "deleteInboxSchema",
        "registerVerificationWait",
        "listVerificationWaits",
        "getVerificationWait",
        "cancelVerificationWait",
        "getAttachmentBrandedLink",
        "getMcpCatalog",
        // The `dairo bucket` command group must keep its backing operations
        // declared — these were added after the projection and were initially
        // missing, so they are pinned here to stop the surface drifting again.
        "listBuckets",
        "createBucket",
        "getBucket",
        "deleteBucket",
        "createBucketObject",
        "finalizeBucketObject",
        "listBucketObjects",
        "getBucketObjectDownloadUrl",
        "deleteBucketObject",
    ] {
        assert!(
            implemented_ops
                .iter()
                .any(|op| op["operationId"].as_str() == Some(required)),
            "CLI-backed operation {required} must be declared"
        );
    }
}

fn operation_key(operation: &serde_json::Value) -> String {
    format!(
        "{} {} {}",
        operation["method"].as_str().unwrap(),
        operation["path"].as_str().unwrap(),
        operation["operationId"].as_str().unwrap()
    )
}
