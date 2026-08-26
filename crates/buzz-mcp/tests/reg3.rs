//! REG-3 stage gate tests — malformed input, empty results, and unit coverage
//! for every clamp/validator the tools rely on. (Live-relay behavior is
//! exercised in the cleaning/hardening stages; here we pin the contracts.)

use buzz_mcp::client::{clamp_limit, extract_d_tag, project_task};

#[test]
fn clamp_limit_contract() {
    assert_eq!(clamp_limit(None), 50);
    assert_eq!(clamp_limit(Some(1)), 1);
    assert_eq!(clamp_limit(Some(0)), 1);
    assert_eq!(clamp_limit(Some(200)), 200);
    assert_eq!(clamp_limit(Some(u32::MAX)), 200);
}

#[test]
fn d_tag_extraction_empty_and_malformed() {
    assert_eq!(extract_d_tag(&serde_json::json!({})), None);
    assert_eq!(
        extract_d_tag(&serde_json::json!({"tags": "not-an-array"})),
        None
    );
    assert_eq!(
        extract_d_tag(&serde_json::json!({"tags": [["x","1"]]})),
        None
    );
    assert_eq!(
        extract_d_tag(&serde_json::json!({"tags": [["d",""]]})),
        Some(String::new())
    );
}

#[test]
fn project_task_null_on_missing_fields() {
    let p = project_task(&serde_json::json!({}));
    for k in ["id", "title", "status", "channel_id"] {
        assert!(p[k].is_null(), "{k} should be null on empty input");
    }
}

#[test]
fn url_encoding_boundaries_via_public_helper() {
    // Pins the injection-relevant boundary set via the crate's own helper:
    // reserved query characters (&, =, ?, #), percent itself, and space must
    // never reach the relay raw.
    assert_eq!(buzz_mcp::urlencode("a&b=c?d#e"), "a%26b%3Dc%3Fd%23e");
    assert_eq!(buzz_mcp::urlencode("100% done"), "100%25%20done");
    assert_eq!(buzz_mcp::urlencode(""), "");
    assert_eq!(buzz_mcp::urlencode("safe-_.~09AZaz"), "safe-_.~09AZaz");
}
