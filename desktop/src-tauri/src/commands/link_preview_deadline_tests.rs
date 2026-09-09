//! Real HTTP transport coverage for the single native operation budget.
use super::*;
use axum::{
    body::Body,
    extract::State,
    http::{Response, Uri},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::{task::JoinHandle, time::Instant};

const TEST_BUDGET: Duration = Duration::from_millis(350);
tokio::task_local! {
    static TEST_OPERATION_TIMEOUT: Duration;
}

pub(super) fn operation_timeout(default: Duration) -> Duration {
    TEST_OPERATION_TIMEOUT
        .try_with(|duration| *duration)
        .unwrap_or(default)
}

pub(super) async fn send_request(
    address: SocketAddr,
    url: &Url,
    accept: &str,
) -> Result<reqwest::Response, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(TRANSPORT_CONNECT_TIMEOUT)
        .read_timeout(TRANSPORT_IDLE_TIMEOUT)
        .build()
        .unwrap()
        .get(format!("http://{address}{}", url.path()))
        .header(ACCEPT, accept)
        .send()
        .await
        .map_err(|error| format!("link preview test request failed: {error}"))
}

#[derive(Default)]
struct Traffic {
    paths: Mutex<Vec<String>>,
    chunks: AtomicUsize,
    live_bodies: AtomicUsize,
}

struct Drip(Arc<Traffic>);

impl Drop for Drip {
    fn drop(&mut self) {
        self.0.live_bodies.fetch_sub(1, Ordering::SeqCst);
    }
}

fn drip_body(traffic: Arc<Traffic>) -> Body {
    traffic.live_bodies.fetch_add(1, Ordering::SeqCst);
    Body::from_stream(futures_util::stream::unfold(
        Drip(traffic),
        |state| async move {
            // Every chunk arrives well inside the production read-idle limit;
            // byte limits are not reached during this bounded regression.
            tokio::time::sleep(Duration::from_millis(20)).await;
            state.0.chunks.fetch_add(1, Ordering::SeqCst);
            Some((Ok::<_, Infallible>(Bytes::from_static(b" ")), state))
        },
    ))
}

struct Server {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn server(router: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    Server {
        address,
        task: tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    }
}

async fn fetch(
    address: SocketAddr,
    href: String,
    id: Option<String>,
) -> Result<Option<LinkPreviewMetadata>, String> {
    METADATA_TEST_SERVER
        .scope(
            address,
            TEST_OPERATION_TIMEOUT.scope(TEST_BUDGET, fetch_link_preview_metadata(href, id)),
        )
        .await
}

async fn wait_until_bodies_drop(traffic: &Traffic) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while traffic.live_bodies.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("deadline must drop the actual response stream");
}

#[tokio::test]
async fn operation_deadline_covers_metadata_oembed_images_redirects_and_cooldown() {
    assert!(PREVIEW_OPERATION_TIMEOUT > TRANSPORT_IDLE_TIMEOUT);
    assert!(PREVIEW_OPERATION_TIMEOUT <= Duration::from_secs(60));
    for stage in [
        "metadata", "oembed", "image", "favicon", "redirect", "cooldown",
    ] {
        let traffic = Arc::new(Traffic::default());
        let endpoint = server(Router::new().fallback(get({
            let traffic = Arc::clone(&traffic);
            move |uri: Uri| {
                let traffic = Arc::clone(&traffic);
                async move {
                    let path = uri.path();
                    traffic.paths.lock().unwrap().push(path.to_string());
                    if stage == "redirect" {
                        tokio::time::sleep(Duration::from_millis(140)).await;
                        let next = match path {
                            "/preview" => "/second",
                            "/second" => "/third",
                            _ => "/last",
                        };
                        return Response::builder()
                            .status(302)
                            .header("location", next)
                            .body(Body::empty())
                            .unwrap();
                    }
                    if path == "/preview" && matches!(stage, "image" | "favicon" | "cooldown") {
                        // The later stage receives only the remaining
                        // operation budget, not another full budget.
                        tokio::time::sleep(Duration::from_millis(220)).await;
                        let html = if stage == "favicon" {
                            "<title>Page</title><link rel='icon' href='/image.png'>"
                        } else {
                            "<title>Page</title><meta property='og:image' content='/image.png'>"
                        };
                        return Response::builder()
                            .header("content-type", "text/html")
                            .body(Body::from(html))
                            .unwrap();
                    }
                    if stage == "cooldown" {
                        return Response::builder()
                            .status(429)
                            .header("retry-after", "1")
                            .body(Body::empty())
                            .unwrap();
                    }
                    Response::builder()
                        .header(
                            "content-type",
                            match stage {
                                "oembed" => "application/json",
                                "image" | "favicon" => "image/png",
                                _ => "text/html",
                            },
                        )
                        .body(drip_body(traffic))
                        .unwrap()
                }
            }
        })))
        .await;
        let href = if stage == "oembed" {
            "https://www.youtube.com/watch?v=fixture".to_string()
        } else {
            format!("https://deadline-{stage}.example/preview")
        };
        let id = format!("deadline-{stage}");
        let request_id = (stage != "oembed").then_some(id.clone());
        let prior_token = cancellation::begin(request_id.as_deref());
        let started = Instant::now();
        let result = tokio::time::timeout(
            TEST_BUDGET + Duration::from_millis(150),
            fetch(endpoint.address, href, request_id),
        )
        .await
        .expect("the operation exceeded its single deadline");
        assert_eq!(
            result,
            Err("link preview operation timed out".to_string()),
            "{stage}"
        );
        wait_until_bodies_drop(&traffic).await;
        let paths = traffic.paths.lock().unwrap().clone();
        assert!(!paths.is_empty());
        if stage == "redirect" {
            assert_eq!(paths, ["/preview", "/second", "/third"]);
        } else if matches!(stage, "image" | "favicon" | "cooldown") {
            assert_eq!(paths, ["/preview", "/image.png"]);
        } else if stage == "oembed" {
            assert_eq!(paths, ["/oembed"]);
        }
        if !matches!(stage, "redirect" | "cooldown") {
            assert!(traffic.chunks.load(Ordering::SeqCst) >= 3, "{stage}");
        }
        // A later owner of this ID must not reuse this completed request token.
        let next = cancellation::begin(Some(&id)).unwrap();
        cancellation::cancel(&id);
        assert!(next.is_cancelled());
        assert!(!prior_token.is_some_and(|token| token.is_cancelled()));
        cancellation::finish(Some(&id));
        println!(
            "PREVIEW_DEADLINE stage={stage} elapsed_ms={} paths={paths:?} live_bodies=0",
            started.elapsed().as_millis()
        );
    }
}

#[tokio::test]
async fn explicit_cancellation_remains_faster_than_the_operation_deadline() {
    let traffic = Arc::new(Traffic::default());
    let endpoint = server(Router::new().route(
        "/preview",
        get({
            let traffic = Arc::clone(&traffic);
            move || {
                let traffic = Arc::clone(&traffic);
                async move {
                    Response::builder()
                        .header("content-type", "text/html")
                        .body(drip_body(traffic))
                        .unwrap()
                }
            }
        }),
    ))
    .await;
    let id = "deadline-explicit-cancel".to_string();
    let request = tokio::spawn(fetch(
        endpoint.address,
        "https://cancel.example/preview".into(),
        Some(id.clone()),
    ));
    tokio::time::timeout(TEST_BUDGET, async {
        while traffic.chunks.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancel_link_preview_metadata(id);
    assert_eq!(
        request.await.unwrap(),
        Err("link preview request cancelled".into())
    );
    wait_until_bodies_drop(&traffic).await;
}

#[derive(Clone)]
struct BridgeState {
    transport: SocketAddr,
    traffic: Arc<Traffic>,
}

/// Cross-language qualification uses the real JS loader and native command;
/// only the IPC boundary and SSRF-pinned transport destination are replaced.
#[tokio::test]
#[ignore = "requires Node and installed desktop JS dependencies"]
async fn renderer_scheduler_advances_after_two_native_slow_drips() {
    let traffic = Arc::new(Traffic::default());
    let transport = server(Router::new().fallback(get({
        let traffic = Arc::clone(&traffic);
        move |uri: Uri| {
            let traffic = Arc::clone(&traffic);
            async move {
                traffic.paths.lock().unwrap().push(uri.path().into());
                Response::builder()
                    .header("content-type", "text/html")
                    .body(if uri.path() == "/fast" {
                        Body::from("<title>Fast preview</title>")
                    } else {
                        drip_body(traffic)
                    })
                    .unwrap()
            }
        }
    })))
    .await;
    let bridge = server(Router::new().route("/invoke", post(
        |State(state): State<BridgeState>, Json(input): Json<serde_json::Value>| async move {
            let command = input["command"].as_str().unwrap();
            let args = &input["args"];
            match command {
                "fetch_link_preview_metadata" => {
                    let result = fetch(state.transport, args["href"].as_str().unwrap().into(),
                        Some(args["requestId"].as_str().unwrap().into())).await;
                    Json(match result {
                        Ok(value) => serde_json::json!({"ok": value}),
                        Err(error) => serde_json::json!({"error": error}),
                    })
                }
                "release_link_preview_metadata" => {
                    release_link_preview_metadata(args["requestId"].as_str().unwrap().into());
                    Json(serde_json::json!({"ok": null}))
                }
                "cancel_link_preview_metadata" => {
                    cancel_link_preview_metadata(args["requestId"].as_str().unwrap().into());
                    Json(serde_json::json!({"ok": null}))
                }
                "fixture_paths" => Json(serde_json::json!({"ok": state.traffic.paths.lock().unwrap().clone()})),
                _ => panic!("unexpected IPC command: {command}"),
            }
        }
    )).with_state(BridgeState { transport: transport.address, traffic: Arc::clone(&traffic) })).await;
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
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    wait_until_bodies_drop(&traffic).await;
    let paths = traffic.paths.lock().unwrap();
    assert_eq!(paths.len(), 3);
    assert_eq!(paths[2], "/fast");
    let mut first = paths[..2].to_vec();
    first.sort();
    assert_eq!(first, ["/slow-one", "/slow-two"]);
    assert!(traffic.chunks.load(Ordering::SeqCst) >= 6);
}
