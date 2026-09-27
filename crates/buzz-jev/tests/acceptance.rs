//! J2 acceptance tests. Fixtures are real recorded systemone responses
//! (`tests/fixtures/*.json`, recorded without the Authorization header).

use buzz_jev::Request;
use serde_json::Value;

const SCORE_TRAP: &str = include_str!("fixtures/score_trap_1_63.json");
const SHAPE_PY: &str = include_str!("fixtures/shape_py.json");

fn fixture(raw: &str) -> Value {
    serde_json::from_str(raw).expect("fixture is JSON")
}

#[test]
fn request_types_round_trip_the_recorded_requests() {
    for raw in [SCORE_TRAP, SHAPE_PY] {
        let wire = fixture(raw)["request"].clone();
        let typed: Request = serde_json::from_value(wire.clone()).expect("request");
        assert_eq!(serde_json::to_value(&typed).expect("serialize"), wire);
    }
}
