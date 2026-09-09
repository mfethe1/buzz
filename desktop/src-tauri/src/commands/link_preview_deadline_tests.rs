//! Real HTTP transport coverage for the single native operation budget.
use super::*;
use axum::{
    body::Body,
    http::{Response, Uri},
    routing::get,
    Router,
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
const REDIRECT_TEST_BUDGET: Duration = Duration::from_secs(1);
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

struct Drip {
    traffic: Arc<Traffic>,
    initial_chunks: usize,
}

impl Drop for Drip {
    fn drop(&mut self) {
        self.traffic.live_bodies.fetch_sub(1, Ordering::SeqCst);
    }
}

fn drip_body(traffic: Arc<Traffic>) -> Body {
    traffic.live_bodies.fetch_add(1, Ordering::SeqCst);
    Body::from_stream(futures_util::stream::unfold(
        Drip {
            traffic,
            initial_chunks: 3,
        },
        |mut state| async move {
            // Establish body progress immediately, then keep the unfinished
            // response alive with a slow drip. Requiring three timer wakes in
            // the image stage's remaining 130 ms made this fixture depend on
            // runner scheduling, despite the operation deadline working.
            if state.initial_chunks > 0 {
                state.initial_chunks -= 1;
            } else {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            state.traffic.chunks.fetch_add(1, Ordering::SeqCst);
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
    fetch_with_budget(address, href, id, TEST_BUDGET).await
}

async fn fetch_with_budget(
    address: SocketAddr,
    href: String,
    id: Option<String>,
    budget: Duration,
) -> Result<Option<LinkPreviewMetadata>, String> {
    METADATA_TEST_SERVER
        .scope(
            address,
            TEST_OPERATION_TIMEOUT.scope(budget, fetch_link_preview_metadata(href, id)),
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
                        // Spend 400 ms across two redirects, then hold the third
                        // response beyond the whole operation budget. The old
                        // 350 ms budget left only 70 ms for all transport and
                        // scheduling overhead before the third request.
                        let delay = if path == "/third" {
                            REDIRECT_TEST_BUDGET * 2
                        } else {
                            Duration::from_millis(200)
                        };
                        tokio::time::sleep(delay).await;
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
        let budget = if stage == "redirect" {
            REDIRECT_TEST_BUDGET
        } else {
            TEST_BUDGET
        };
        let result = tokio::time::timeout(
            budget + Duration::from_millis(150),
            fetch_with_budget(endpoint.address, href, request_id, budget),
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

#[path = "link_preview_scheduler_tests.rs"]
mod scheduler;
