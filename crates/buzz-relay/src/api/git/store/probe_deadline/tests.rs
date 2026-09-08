use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, Response, StatusCode, Uri};
use axum::routing::any;
use axum::Router;
use tokio::sync::Mutex;

use super::super::{GitStore, ProbeConfig, StoreError};

#[derive(Default)]
struct Backend {
    objects: Mutex<HashMap<String, (Bytes, String)>>,
    requests: AtomicUsize,
    cas_requests: AtomicUsize,
    stall_first_cas: bool,
}

async fn object(
    State(backend): State<Arc<Backend>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    backend.requests.fetch_add(1, Ordering::SeqCst);
    if method == Method::PUT && headers.contains_key(header::IF_MATCH) {
        let index = backend.cas_requests.fetch_add(1, Ordering::SeqCst);
        if backend.stall_first_cas && index == 0 {
            std::future::pending::<()>().await;
        }
    }
    let mut objects = backend.objects.lock().await;
    let key = uri.path().to_string();
    let reply = |status, body, tag: Option<&str>| {
        let mut response = Response::builder().status(status);
        if let Some(tag) = tag {
            response = response.header(header::ETAG, tag);
        }
        response.body(Body::from(body)).expect("fixture response")
    };
    match method {
        Method::PUT => {
            let existing = objects.get(&key);
            if (headers.contains_key(header::IF_NONE_MATCH) && existing.is_some())
                || headers.get(header::IF_MATCH).is_some_and(|condition| {
                    existing.is_none_or(|(_, tag)| condition.as_bytes() != tag.as_bytes())
                })
            {
                return reply(StatusCode::PRECONDITION_FAILED, Bytes::new(), None);
            }
            let tag = format!("\"{}\"", GitStore::digest_hex(&body));
            objects.insert(key, (body, tag.clone()));
            reply(StatusCode::OK, Bytes::new(), Some(&tag))
        }
        Method::GET => match objects.get(&key) {
            Some((body, tag)) => reply(StatusCode::OK, body.clone(), Some(tag)),
            None => reply(StatusCode::NOT_FOUND, Bytes::new(), None),
        },
        Method::DELETE => {
            objects.remove(&key);
            reply(StatusCode::NO_CONTENT, Bytes::new(), None)
        }
        _ => reply(StatusCode::METHOD_NOT_ALLOWED, Bytes::new(), None),
    }
}

async fn start_backend(stall: bool) -> (GitStore, Arc<Backend>, tokio::task::JoinHandle<()>) {
    let state = Arc::new(Backend {
        stall_first_cas: stall,
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("local fixture listener");
    let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
    let app = Router::new()
        .route("/{*key}", any(object))
        .with_state(Arc::clone(&state));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("fixture server");
    });
    let store = GitStore::new(
        &endpoint,
        "fixture-access",
        "fixture-secret",
        "probe-test",
        "us-east-1",
        buzz_media::config::S3AddressingStyle::Path,
    )
    .expect("fixture store");
    (store, state, server)
}

#[tokio::test]
async fn stalled_racer_fails_the_total_deadline_instead_of_admitting_backend() {
    let (store, state, server) = start_backend(true).await;
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        store.run_conformance_probe(ProbeConfig {
            race_width: 3,
            race_rounds: 1,
            total_timeout: Duration::from_millis(250),
        }),
    )
    .await;
    server.abort();
    let _ = server.await;
    let Err(StoreError::Probe(failure)) = result.expect("production deadline must finish first")
    else {
        panic!("a pending racer must never become successful admission");
    };
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(state.cas_requests.load(Ordering::SeqCst), 3);
    assert_eq!(failure.phase, "deadline");
    assert!(failure.reason.contains("backend not admitted"));
    eprintln!("STALLED_S3_PROBE_DENIED: {failure}");
}

#[tokio::test]
async fn responsive_backend_still_completes_all_conformance_phases() {
    let (store, state, server) = start_backend(false).await;
    let result = store
        .run_conformance_probe(ProbeConfig {
            race_width: 4,
            race_rounds: 2,
            // rust-s3 retries classified 412 responses once after a one-second
            // backoff. Four race phases therefore need more than four seconds.
            total_timeout: Duration::from_secs(10),
        })
        .await;
    server.abort();
    let _ = server.await;
    let report = result.expect("responsive conditional-write backend");
    assert_eq!(report.race_width, 4);
    assert_eq!(report.race_rounds, 2);
    assert_eq!(report.transport_drops, 0);
    // Two four-writer races, six 412 retries, and two ETag consistency writes.
    assert_eq!(state.cas_requests.load(Ordering::SeqCst), 16);
    eprintln!("RESPONSIVE_S3_PROBE_ADMITTED: {report:?}");
}

#[tokio::test]
async fn zero_deadline_is_invalid_without_sending_backend_requests() {
    let (store, state, server) = start_backend(false).await;
    let result = store
        .run_conformance_probe(ProbeConfig {
            total_timeout: Duration::ZERO,
            ..ProbeConfig::default()
        })
        .await;
    server.abort();
    let _ = server.await;
    assert_eq!(state.requests.load(Ordering::SeqCst), 0);
    assert!(matches!(result, Err(StoreError::Probe(failure)) if failure.phase == "config"));
}
