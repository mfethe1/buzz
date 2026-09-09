//! Real native/renderer qualification with transport-observed readiness.
use super::*;
use axum::{extract::State, routing::post, Json};
use std::sync::atomic::AtomicBool;
use tokio::sync::Notify;

const SCHEDULER_BUDGET: Duration = Duration::from_secs(1);
const READINESS_BUDGET: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Arrival {
    Normal,
    Reversed,
    DelayedUntilReadiness,
    Missing,
    ExpiredBeforeReadiness,
}

struct Activity {
    traffic: Arc<Traffic>,
    completed: AtomicUsize,
    in_flight: AtomicUsize,
    changed: Notify,
    dispatch_released: AtomicBool,
    arrival: Arrival,
}

impl Activity {
    fn paths(&self) -> Vec<String> {
        self.traffic.paths.lock().unwrap().clone()
    }

    /// Capture current liveness, not just historical arrivals. A terminal
    /// native request can never satisfy readiness, even if both paths were seen.
    fn snapshot(&self) -> Result<Option<serde_json::Value>, String> {
        let paths = self.paths();
        let completed = self.completed.load(Ordering::SeqCst);
        if completed != 0 {
            return Err("fixture readiness followed native settlement".into());
        }
        if paths.len() > 2 || paths.iter().any(|path| path == "/fast") {
            return Err("third request reached transport before readiness".into());
        }
        let live = self.traffic.live_bodies.load(Ordering::SeqCst);
        let mut sorted = paths.clone();
        sorted.sort();
        Ok(
            (live == 2 && sorted == ["/slow-one", "/slow-two"]).then(|| {
                serde_json::json!({"ready": true, "paths": paths,
                "liveBodies": live, "completed": completed})
            }),
        )
    }

    async fn wait_for(&self, predicate: impl Fn() -> bool) -> Result<(), String> {
        tokio::time::timeout(READINESS_BUDGET, async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if predicate() {
                    return;
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| "fixture readiness timed out".into())
    }

    async fn ready(&self) -> Result<serde_json::Value, String> {
        if self.arrival == Arrival::DelayedUntilReadiness {
            // A deterministic dispatch gate: the second request cannot reach
            // the transport until the renderer asks for actual readiness.
            self.wait_for(|| self.paths().iter().any(|p| p == "/slow-one"))
                .await?;
            self.dispatch_released.store(true, Ordering::SeqCst);
            self.changed.notify_waiters();
        }
        if self.arrival == Arrival::ExpiredBeforeReadiness {
            self.wait_for(|| self.completed.load(Ordering::SeqCst) != 0)
                .await?;
        }
        tokio::time::timeout(READINESS_BUDGET, async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if let Some(snapshot) = self.snapshot()? {
                    return Ok(snapshot);
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| "fixture readiness timed out".to_string())?
    }
}

struct NativeCall<'a>(&'a Activity);
impl Drop for NativeCall<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }
}

#[derive(Clone)]
struct BridgeState {
    transport: SocketAddr,
    activity: Arc<Activity>,
}

async fn native_fetch(
    state: &BridgeState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    state.activity.in_flight.fetch_add(1, Ordering::SeqCst);
    let _call = NativeCall(&state.activity);
    let href = args["href"].as_str().unwrap().to_string();
    let path = Url::parse(&href).unwrap().path().to_string();
    match (state.activity.arrival, path.as_str()) {
        (Arrival::Reversed, "/slow-one") => {
            state
                .activity
                .wait_for(|| state.activity.paths().iter().any(|p| p == "/slow-two"))
                .await?;
        }
        (Arrival::DelayedUntilReadiness | Arrival::Missing, "/slow-two") => {
            state
                .activity
                .wait_for(|| state.activity.dispatch_released.load(Ordering::SeqCst))
                .await?;
        }
        _ => {}
    }
    // One budget starts once per actual native operation. No readiness request
    // or metadata stage resets or pauses it.
    let result = METADATA_TEST_SERVER
        .scope(
            state.transport,
            TEST_OPERATION_TIMEOUT.scope(
                SCHEDULER_BUDGET,
                fetch_link_preview_metadata(href, Some(args["requestId"].as_str().unwrap().into())),
            ),
        )
        .await;
    state.activity.completed.fetch_add(1, Ordering::SeqCst);
    state.activity.changed.notify_waiters();
    result.map(|value| serde_json::to_value(value).unwrap())
}

async fn invoke(
    State(state): State<BridgeState>,
    Json(input): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let args = &input["args"];
    let result = match input["command"].as_str().unwrap() {
        "fetch_link_preview_metadata" => native_fetch(&state, args).await,
        "fixture_wait_ready" => state.activity.ready().await,
        "fixture_ready_ack" => state.activity.snapshot().and_then(|value| {
            value.ok_or_else(|| "fixture lost readiness before acknowledgement".into())
        }),
        "release_link_preview_metadata" => {
            release_link_preview_metadata(args["requestId"].as_str().unwrap().into());
            Ok(serde_json::Value::Null)
        }
        "cancel_link_preview_metadata" => {
            cancel_link_preview_metadata(args["requestId"].as_str().unwrap().into());
            Ok(serde_json::Value::Null)
        }
        other => Err(format!("unexpected fixture command: {other}")),
    };
    Json(match result {
        Ok(value) => serde_json::json!({"ok": value}),
        Err(error) => serde_json::json!({"error": error}),
    })
}

/// Uses actual production TypeScript scheduling and native slow-drip HTTP.
/// Only the IPC boundary and SSRF-pinned destination use an owned fixture.
#[tokio::test]
#[ignore = "requires Node and installed desktop JS dependencies"]
async fn renderer_scheduler_advances_after_two_native_slow_drips() {
    for arrival in [
        Arrival::Normal,
        Arrival::Reversed,
        Arrival::DelayedUntilReadiness,
        Arrival::Missing,
        Arrival::ExpiredBeforeReadiness,
    ] {
        let activity = Arc::new(Activity {
            traffic: Arc::new(Traffic::default()),
            completed: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            changed: Notify::new(),
            dispatch_released: AtomicBool::new(false),
            arrival,
        });
        let transport = server(Router::new().fallback(get({
            let activity = Arc::clone(&activity);
            move |uri: Uri| {
                let activity = Arc::clone(&activity);
                async move {
                    activity
                        .traffic
                        .paths
                        .lock()
                        .unwrap()
                        .push(uri.path().into());
                    let body = if uri.path() == "/fast" {
                        Body::from("<title>Fast preview</title>")
                    } else {
                        drip_body(Arc::clone(&activity.traffic))
                    };
                    activity.changed.notify_waiters();
                    Response::builder()
                        .header("content-type", "text/html")
                        .body(body)
                        .unwrap()
                }
            }
        })))
        .await;
        let bridge = server(
            Router::new()
                .route("/invoke", post(invoke))
                .with_state(BridgeState {
                    transport: transport.address,
                    activity: Arc::clone(&activity),
                }),
        )
        .await;
        let desktop = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let output = tokio::process::Command::new("node")
            .args([
                "--import",
                "./test-loader.mjs",
                "--experimental-strip-types",
                "./scripts/qualify-link-preview-deadline.mjs",
            ])
            .arg(format!("http://{}/invoke", bridge.address))
            .current_dir(desktop)
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(Duration::from_secs(10), output)
            .await
            .unwrap()
            .unwrap();
        let passes = !matches!(arrival, Arrival::Missing | Arrival::ExpiredBeforeReadiness);
        assert_eq!(
            output.status.success(),
            passes,
            "{arrival:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        activity
            .wait_for(|| activity.in_flight.load(Ordering::SeqCst) == 0)
            .await
            .expect("all owned bridge calls must finish before fixture teardown");
        wait_until_bodies_drop(&activity.traffic).await;
        let paths = activity.paths();
        if passes {
            assert_eq!(paths.len(), 3);
            assert_eq!(paths[2], "/fast");
            let mut first = paths[..2].to_vec();
            first.sort();
            assert_eq!(first, ["/slow-one", "/slow-two"]);
            if arrival == Arrival::Reversed {
                assert_eq!(paths[0], "/slow-two");
            }
            assert!(activity.traffic.chunks.load(Ordering::SeqCst) >= 6);
            assert_eq!(activity.completed.load(Ordering::SeqCst), 3);
        } else {
            assert!(String::from_utf8_lossy(&output.stderr)
                .contains("fixture readiness followed native settlement"));
            assert!(!String::from_utf8_lossy(&output.stdout).contains("\"result\":\"PASS\""));
            if arrival == Arrival::Missing {
                assert!(!paths.iter().any(|path| path == "/slow-two"));
            } else {
                assert!(paths.iter().any(|path| path == "/slow-one"));
                assert!(paths.iter().any(|path| path == "/slow-two"));
            }
            println!(
                "PREVIEW_READINESS_DENIED {arrival:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        println!(
            "PREVIEW_READINESS arrival={arrival:?} expected_pass={passes} paths={paths:?} live_bodies=0 in_flight=0 {}",
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
}
