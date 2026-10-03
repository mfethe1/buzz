//! J2 acceptance tests. Fixtures are real recorded systemone responses
//! (`tests/fixtures/*.json`, recorded without the Authorization header).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use buzz_jev::{argmax_index, Answer, Client, JevError, Judgment, Request, TIMEOUT};
use serde_json::Value;

const SCORE_TRAP: &str = include_str!("fixtures/score_trap_1_63.json");
const SHAPE_PY: &str = include_str!("fixtures/shape_py.json");

fn fixture(raw: &str) -> Value {
    serde_json::from_str(raw).expect("fixture is JSON")
}

fn parse(response: &Value) -> Result<Judgment, JevError> {
    Judgment::from_slice(&serde_json::to_vec(response).expect("serialize"))
}

fn anomaly_in(result: Result<Judgment, JevError>) -> String {
    match result {
        Err(JevError::ShapeAnomaly { question, .. }) => question,
        other => panic!("expected ShapeAnomaly, got {other:?}"),
    }
}

// (1) Score trap: score=1.63, probabilities peak at index 2.
#[test]
fn score_trap_returns_argmax_index_not_score() {
    let recorded = fixture(SCORE_TRAP);
    let spec = &recorded["response"]["answers"]["actionable_specificity"];
    assert_eq!(spec["score"], 1.63, "fixture must be the recorded trap");

    let probabilities: BTreeMap<String, f64> =
        serde_json::from_value(spec["probabilities"].clone()).expect("probabilities");
    let (index, mass) = argmax_index("actionable_specificity", &probabilities).expect("argmax");
    assert_eq!(index, 2);
    assert_eq!(mass, 0.63);

    let judgment = parse(&recorded["response"]).expect("recorded response parses");
    assert_eq!(
        judgment.answers["actionable_specificity"],
        Answer::Score {
            index: 2,
            mass: 0.63
        }
    );
    let request: Request = serde_json::from_value(recorded["request"].clone()).expect("request");
    judgment
        .check_against(&request)
        .expect("every asked question answered");
}

// (1) `.score` is never read: no accessor exists (see the compile_fail doctest
// on `Answer`), and the parse is identical whatever the wire `score` holds.
#[test]
fn wire_score_is_never_read() {
    let recorded = fixture(SCORE_TRAP);
    let baseline = parse(&recorded["response"]).expect("baseline");
    for poison in [
        Value::from(0),
        Value::from(-7.5),
        Value::from("garbage"),
        Value::Null,
    ] {
        let mut response = recorded["response"].clone();
        response["answers"]["actionable_specificity"]["score"] = poison.clone();
        assert_eq!(
            parse(&response).expect("parses"),
            baseline,
            "score={poison}"
        );
    }
    let mut response = recorded["response"].clone();
    let spec = response["answers"]["actionable_specificity"]
        .as_object_mut()
        .expect("object");
    spec.remove("score");
    assert_eq!(parse(&response).expect("parses without score"), baseline);
}

// (2) `.choice` that is not the argmax of its own probabilities.
#[test]
fn choice_not_argmax_is_shape_anomaly() {
    let mut response = fixture(SHAPE_PY)["response"].clone();
    assert_eq!(response["answers"]["route"]["probabilities"]["infra"], 1.0);
    response["answers"]["route"]["choice"] = Value::from("security");
    assert_eq!(anomaly_in(parse(&response)), "route");

    response["answers"]["route"]["choice"] = Value::from("not-an-option");
    assert_eq!(anomaly_in(parse(&response)), "route");
}

#[test]
fn out_of_range_values_are_shape_anomalies() {
    let recorded = fixture(SHAPE_PY)["response"].clone();
    let mut response = recorded.clone();
    response["answers"]["warrants_agent_action"]["noul"] = Value::from(1.5);
    assert_eq!(anomaly_in(parse(&response)), "warrants_agent_action");

    // A score winner that is not a level index is never clamped to one.
    let mut response = fixture(SCORE_TRAP)["response"].clone();
    response["answers"]["actionable_specificity"]["probabilities"] =
        serde_json::json!({"0": 0.1, "1": 0.2, "7": 0.7});
    assert_eq!(anomaly_in(parse(&response)), "actionable_specificity");
}

#[test]
fn noul_choice_and_usage_parse_from_recorded_shape() {
    let recorded = fixture(SHAPE_PY);
    let judgment = parse(&recorded["response"]).expect("parses");
    assert_eq!(judgment.model, "jev-1.13.0");
    assert_eq!(
        judgment.answers["warrants_agent_action"],
        Answer::Noul { p_yes: 0.69 }
    );
    assert_eq!(
        judgment.answers["route"],
        Answer::Choice {
            option: "infra".into(),
            mass: 1.0
        }
    );
    assert_eq!(judgment.usage.input_tokens, 407);
    assert_eq!(judgment.usage.output_tokens, 60);
}

#[test]
fn request_types_round_trip_the_recorded_requests() {
    for raw in [SCORE_TRAP, SHAPE_PY] {
        let wire = fixture(raw)["request"].clone();
        let typed: Request = serde_json::from_value(wire.clone()).expect("request");
        assert_eq!(serde_json::to_value(&typed).expect("serialize"), wire);
    }
}

#[test]
fn missing_answer_fails_the_request_check() {
    let recorded = fixture(SHAPE_PY);
    let mut response = recorded["response"].clone();
    response["answers"]
        .as_object_mut()
        .expect("object")
        .remove("route");
    let request: Request = serde_json::from_value(recorded["request"].clone()).expect("request");
    let judgment = parse(&response).expect("parses");
    assert!(matches!(
        judgment.check_against(&request),
        Err(JevError::ShapeAnomaly { question, .. }) if question == "route"
    ));
}

#[test]
fn blank_key_is_a_loud_error() {
    assert!(matches!(
        Client::new(buzz_jev::ENDPOINT, "  "),
        Err(JevError::MissingKey)
    ));
}

// (3) A real local listener that accepts and then never responds.
#[tokio::test]
async fn silent_server_times_out_within_2_1_s() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });

    let client = Client::new(format!("http://{addr}/v1/systemone"), "test-key").expect("client");
    let request: Request =
        serde_json::from_value(fixture(SHAPE_PY)["request"].clone()).expect("request");

    let started = Instant::now();
    let result = client.judge(&request).await;
    let elapsed = started.elapsed();

    assert!(matches!(result, Err(JevError::Timeout)), "got {result:?}");
    assert!(elapsed >= TIMEOUT, "fired early: {elapsed:?}");
    assert!(
        elapsed < Duration::from_millis(2100),
        "too slow: {elapsed:?}"
    );
}

// (4) Live smoke: JEV_LIVE=1 sends the shape.py request to production.
#[tokio::test]
async fn live_shape_py_smoke() {
    if std::env::var("JEV_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set JEV_LIVE=1 and JEV_API_KEY to run");
        return;
    }
    let client = Client::from_env().expect("JEV_LIVE=1 requires JEV_API_KEY");
    let request: Request =
        serde_json::from_value(fixture(SHAPE_PY)["request"].clone()).expect("request");
    let judgment = client.judge(&request).await.expect("live call");

    assert!(
        judgment.model.starts_with("jev-"),
        "model {}",
        judgment.model
    );
    assert!(matches!(
        judgment.answers["warrants_agent_action"],
        Answer::Noul { .. }
    ));
    match &judgment.answers["route"] {
        Answer::Choice { option, .. } => {
            assert!(["infra", "security", "abstain"].contains(&option.as_str()));
        }
        other => panic!("route is not a choice: {other:?}"),
    }
    assert!(judgment.usage.input_tokens > 0 && judgment.usage.output_tokens > 0);
}
