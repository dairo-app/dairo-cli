#[test]
fn implemented_operations_match_live_contract() {
    let live: serde_json::Value =
        serde_json::from_str(include_str!("../contract/live-operations.json")).unwrap();
    let implemented: serde_json::Value =
        serde_json::from_str(include_str!("../contract/implemented-operations.json")).unwrap();

    assert_eq!(
        live, implemented,
        "implemented operation contract drifted from live contract projection"
    );
}
