//! HTTP/2 serving contract for the daemon's listeners.
//!
//! The daemon serves plain HTTP through `axum::serve` with axum's `http2`
//! feature enabled: hyper-util's auto connection builder detects the protocol
//! per connection, so HTTP/1.1 callers and HTTP/2 callers speaking h2c prior
//! knowledge share one listener without any configuration or usage change.
//! These tests pin that contract: both protocols answer on the same port,
//! SSE turn-style streams survive the protocol switch, and a single h2
//! client multiplexes concurrent streams over one connection — verified by
//! observing that both server-side requests arrived from the same remote
//! address, not merely by them overlapping in time.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::Router;
use futures_util::Stream;
use reqwest::{Client, StatusCode, Version};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Barrier};
use tokio::time::timeout;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Event payload pushed per SSE tick; three ticks close the stream.
const TICK_COUNT: usize = 3;

/// Mirror of the daemon's serving path (`axum::serve` on a loopback listener)
/// with the two response shapes that matter for protocol compatibility: a
/// plain JSON GET and the SSE POST the turn endpoint answers with. Returns
/// the bound address; callers build the `http://` base the same way the
/// router-test helpers do.
async fn spawn_serving_router() -> SocketAddr {
    let router = Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(json!({"status": "ok"})) }),
        )
        .route("/stream", post(sse_stream));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve router");
    });
    address
}

/// SSE stream shaped like the daemon's turn endpoint: events flow from a
/// spawned producer through an unbounded channel, with the same keep-alive
/// axum `Sse` default the router applies.
async fn sse_stream() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (events, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        for index in 0..TICK_COUNT {
            events
                .send(Ok(Event::default()
                    .event("tick")
                    .data(format!("tick-{index}"))))
                .expect("sse receiver alive");
        }
    });
    Sse::new(UnboundedReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

/// Shared state behind [`gated_sse_stream`]: the barrier parks both requests
/// once they are in flight, and `remotes` records the client address each
/// request arrived on. Multiplexing means both requests ride one TCP
/// connection — i.e. one distinct remote address, which the test asserts.
#[derive(Clone)]
struct GatedStreamState {
    barrier: Arc<Barrier>,
    remotes: Arc<std::sync::Mutex<Vec<SocketAddr>>>,
}

/// SSE stream whose producer parks on the barrier until two requests are in
/// flight at once: only a multiplexing client can complete both streams.
async fn gated_sse_stream(
    State(state): State<GatedStreamState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    state
        .remotes
        .lock()
        .expect("remotes mutex poisoned")
        .push(remote);
    let barrier = state.barrier.clone();
    let (events, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        events
            .send(Ok(Event::default().event("tick").data("opened")))
            .expect("sse receiver alive");
        barrier.wait().await;
        events
            .send(Ok(Event::default().event("tick").data("released")))
            .expect("sse receiver alive");
    });
    Sse::new(UnboundedReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

fn h2c_client() -> Client {
    Client::builder()
        .http2_prior_knowledge()
        .build()
        .expect("h2c prior-knowledge client")
}

/// Consume an SSE response and return its `data:` payloads in order.
async fn collect_sse_ticks(response: reqwest::Response) -> Vec<String> {
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("text/event-stream"),
        "unexpected content type: {content_type}"
    );
    let body = response.text().await.expect("sse body");
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(ToString::to_string)
        .collect()
}

#[tokio::test]
async fn one_listener_answers_http2_prior_knowledge_and_http1() {
    let address = spawn_serving_router().await;
    let base = format!("http://{address}");

    let response = h2c_client()
        .get(format!("{base}/health"))
        .send()
        .await
        .expect("h2c health request");
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(response.status(), StatusCode::OK);
    let health: serde_json::Value = response.json().await.expect("h2c health body");
    assert_eq!(health["status"], "ok");

    // The same listener keeps answering plain HTTP/1.1 callers unchanged.
    let response = Client::new()
        .get(format!("{base}/health"))
        .send()
        .await
        .expect("http1 health request");
    assert_eq!(response.version(), Version::HTTP_11);
    assert_eq!(response.status(), StatusCode::OK);
    let health: serde_json::Value = response.json().await.expect("http1 health body");
    assert_eq!(health["status"], "ok");
}

#[tokio::test]
async fn sse_turn_stream_survives_on_both_protocols() {
    let address = spawn_serving_router().await;
    let base = format!("http://{address}");

    let response = h2c_client()
        .post(format!("{base}/stream"))
        .send()
        .await
        .expect("h2c stream request");
    assert_eq!(response.version(), Version::HTTP_2);
    let ticks = collect_sse_ticks(response).await;
    assert_eq!(
        ticks,
        ["tick-0", "tick-1", "tick-2"],
        "h2c must deliver the full SSE event sequence"
    );

    let response = Client::new()
        .post(format!("{base}/stream"))
        .send()
        .await
        .expect("http1 stream request");
    assert_eq!(response.version(), Version::HTTP_11);
    let ticks = collect_sse_ticks(response).await;
    assert_eq!(
        ticks,
        ["tick-0", "tick-1", "tick-2"],
        "http1 must keep delivering the full SSE event sequence"
    );
}

#[tokio::test]
async fn h2c_client_multiplexes_concurrent_sse_streams() {
    let state = GatedStreamState {
        barrier: Arc::new(Barrier::new(2)),
        remotes: Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    let router = Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(json!({"status": "ok"})) }),
        )
        .route("/gated-stream", post(gated_sse_stream))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        // ConnectInfo is what lets the handler observe per-connection
        // remote addresses; multiplexing means both gated requests must
        // report the same one.
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve router");
    });
    let base = format!("http://{address}");

    let client = h2c_client();
    // Warm the pool with a fully-consumed request first, so the gated
    // requests below reuse this pooled connection instead of each racing a
    // fresh dial: the assertion then pins pool reuse / multiplexing
    // deterministically instead of depending on how concurrent first dials
    // settle.
    let warmup = client
        .get(format!("{base}/health"))
        .send()
        .await
        .expect("warmup health request");
    assert_eq!(warmup.status(), StatusCode::OK);
    warmup
        .text()
        .await
        .expect("warmup body must be consumed so the connection returns to the pool");

    let (first, second) = timeout(Duration::from_secs(10), async {
        tokio::join!(
            collect_sse_ticks(
                client
                    .post(format!("{base}/gated-stream"))
                    .send()
                    .await
                    .expect("first gated stream")
            ),
            collect_sse_ticks(
                client
                    .post(format!("{base}/gated-stream"))
                    .send()
                    .await
                    .expect("second gated stream")
            ),
        )
    })
    .await
    .expect("both concurrent h2 streams must finish; a client that serialized them would stall on the barrier");
    assert_eq!(first, ["opened", "released"]);
    assert_eq!(second, ["opened", "released"]);

    // The multiplexing claim itself: both gated requests were served over
    // one connection, so the handler observed one and the same remote
    // address twice. Two separate connections would report distinct source
    // ports even from the same client process.
    let remotes = state.remotes.lock().expect("remotes mutex poisoned");
    assert_eq!(
        remotes.len(),
        2,
        "both gated requests must have been served"
    );
    assert_eq!(
        remotes[0], remotes[1],
        "both concurrent streams must ride one multiplexed connection (saw {remotes:?})"
    );
}
