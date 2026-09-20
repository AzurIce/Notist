use notist_analysis::debug::{analyze, base64_decode, base64_encode};
#[test]
fn snapshot_describes_evaluation_and_items() {
    let request=serde_json::json!({"entry":"README.notc","files":{"README.notc":"let f = (body: Content) => item(\"section\", (body: body)); f[hello];"}}).to_string();
    let mut a = analyze(&request);
    let mut b = analyze(&request);
    a["platform"] = serde_json::Value::Null;
    b["platform"] = serde_json::Value::Null;
    assert_eq!(a, b);
    assert!(a.get("normalization").is_none());
    assert_eq!(a["result"]["content"]["sequence"][0]["item"], "section");
    assert!(!a["evaluation"]["events"].as_array().unwrap().is_empty());
    assert!(a["result"]["diagnostics"].as_array().unwrap().is_empty());
}
#[test]
fn invalid_requests_and_binary_encoding() {
    assert!(
        !analyze("{")["result"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for bytes in [vec![], vec![0], vec![255, 1], vec![3, 4, 5, 6]] {
        assert_eq!(base64_decode(&base64_encode(&bytes)).unwrap(), bytes);
    }
    assert!(base64_decode("=aaa").is_err());
}
