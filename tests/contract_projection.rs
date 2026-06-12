#[test]
fn implemented_operations_are_an_honest_live_subset() {
    let live: serde_json::Value =
        serde_json::from_str(include_str!("../contract/live-operations.json")).unwrap();
    let implemented: serde_json::Value =
        serde_json::from_str(include_str!("../contract/implemented-operations.json")).unwrap();

    assert_ne!(
        live["operationCount"], implemented["operationCount"],
        "the CLI contract must not claim full live API parity by copying the live projection"
    );
    assert_eq!(implemented["coverage"], "implemented-subset");

    let live_ops = live["operations"].as_array().unwrap();
    let implemented_ops = implemented["operations"].as_array().unwrap();
    let live_keys: std::collections::BTreeSet<_> = live_ops.iter().map(operation_key).collect();

    for op in implemented_ops {
        let source = op
            .get("contractSource")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("openapi");
        if source == "openapi" {
            assert!(
                live_keys.contains(&operation_key(op)),
                "implemented OpenAPI operation is absent from live projection: {op:?}"
            );
        }
    }

    for required in [
        "getInboxSchema",
        "setInboxSchema",
        "deleteInboxSchema",
        "registerVerificationWait",
        "listVerificationWaits",
        "getVerificationWait",
        "cancelVerificationWait",
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
