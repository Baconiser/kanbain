use std::sync::Mutex as StdMutex;

use axum::{Json, http::HeaderMap, routing::post};
use tokio::sync::Notify;

use super::*;

#[derive(Default)]
struct RecordingEvents {
    calls: StdMutex<Vec<ModelCallRecord>>,
    exhausted: StdMutex<Vec<(Uuid, f64)>>,
    notify: Notify,
}

#[async_trait]
impl ProxyEvents for RecordingEvents {
    async fn model_call(&self, record: ModelCallRecord) {
        self.calls.lock().unwrap().push(record);
        self.notify.notify_waiters();
    }

    async fn budget_exhausted(&self, run_id: Uuid, spent_usd: f64) {
        self.exhausted.lock().unwrap().push((run_id, spent_usd));
    }
}

impl RecordingEvents {
    async fn wait_for_calls(&self, n: usize) -> Vec<ModelCallRecord> {
        for _ in 0..100 {
            if self.calls.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        self.calls.lock().unwrap().clone()
    }
}

/// A fake Anthropic API: checks the injected key, streams a message whose
/// text is `chunks` deltas of 400 characters each.
async fn mock_upstream(chunks: usize) -> String {
    let app = Router::new()
        .route(
            "/v1/messages",
            post(
                move |headers: HeaderMap, Json(body): Json<Value>| async move {
                    assert_eq!(headers.get("x-api-key").unwrap(), "real-key");
                    assert!(headers.get("authorization").is_none());
                    let model = body["model"].as_str().unwrap().to_string();
                    if body["stream"] != json!(true) {
                        return Json(json!({
                            "type": "message",
                            "model": model,
                            "usage": {"input_tokens": 1000, "output_tokens": 500,
                                      "cache_read_input_tokens": 0}
                        }))
                        .into_response();
                    }
                    let mut sse = format!(
                        "event: message_start\ndata: {}\n\n",
                        json!({"type": "message_start", "message": {"model": model,
                        "usage": {"input_tokens": 1000, "output_tokens": 1,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}})
                    );
                    let text = "x".repeat(400);
                    for _ in 0..chunks {
                        sse.push_str(&format!(
                            "event: content_block_delta\ndata: {}\n\n",
                            json!({"type": "content_block_delta", "index": 0,
                               "delta": {"type": "text_delta", "text": text}})
                        ));
                    }
                    sse.push_str(&format!(
                        "event: message_delta\ndata: {}\n\n",
                        json!({"type": "message_delta", "usage": {"output_tokens": chunks * 100}})
                    ));
                    let stream = futures::stream::iter(
                        sse.into_bytes()
                            .chunks(512)
                            .map(|c| Ok::<_, std::io::Error>(Bytes::copy_from_slice(c)))
                            .collect::<Vec<_>>(),
                    );
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(stream))
                        .unwrap()
                },
            ),
        )
        .route(
            "/v1/messages/count_tokens",
            post(|| async { Json(json!({"input_tokens": 12})) }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}

async fn start_proxy(upstream: &str) -> (ModelProxy, Arc<RecordingEvents>, String) {
    let events = Arc::new(RecordingEvents::default());
    let proxy = ModelProxy::new(
        PriceTable::default(),
        Arc::new(StaticCredentials {
            anthropic: Some(StaticCredentials::anthropic_api_key(upstream, "real-key")),
            openai: None,
        }),
        events.clone(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(proxy.clone().serve(listener));
    (proxy, events, format!("http://{addr}"))
}

fn message_request(stream: bool) -> Value {
    json!({"model": "claude-sonnet-5-5", "max_tokens": 100, "stream": stream,
           "messages": [{"role": "user", "content": "hi"}]})
}

#[tokio::test]
async fn rejects_missing_and_unknown_tokens() {
    let upstream = mock_upstream(1).await;
    let (_proxy, _events, url) = start_proxy(&upstream).await;
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{url}/v1/messages"))
        .json(&message_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let res = client
        .post(format!("{url}/v1/messages"))
        .header("x-api-key", "kbr-nope")
        .json(&message_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn meters_json_and_stream_calls() {
    let upstream = mock_upstream(3).await;
    let (proxy, events, url) = start_proxy(&upstream).await;
    let run = Uuid::new_v4();
    let token = proxy.issue_token(run, Some(5.0), 0.0);
    let client = reqwest::Client::new();

    let res = client
        .post(format!("{url}/v1/messages"))
        .header("x-api-key", &token)
        .json(&message_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let res = client
        .post(format!("{url}/v1/messages"))
        .bearer_auth(&token)
        .json(&message_request(true))
        .send()
        .await
        .unwrap();
    let body = res.text().await.unwrap();
    assert!(body.contains("message_delta"));

    let res = client
        .post(format!("{url}/v1/messages/count_tokens"))
        .header("x-api-key", &token)
        .json(&message_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let calls = events.wait_for_calls(3).await;
    assert_eq!(calls.len(), 3);
    let json_call = calls
        .iter()
        .find(|c| !c.streamed && c.path == "/v1/messages")
        .unwrap();
    assert_eq!(json_call.run_id, run);
    assert_eq!(json_call.model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(json_call.usage.output_tokens, 500);
    // 1000 * $2/M + 500 * $10/M
    assert!((json_call.cost_usd.unwrap() - 0.007).abs() < 1e-9);
    let stream_call = calls.iter().find(|c| c.streamed).unwrap();
    assert_eq!(stream_call.usage.output_tokens, 300);
    assert!(!stream_call.cut_off);
    let count_call = calls
        .iter()
        .find(|c| c.path.ends_with("count_tokens"))
        .unwrap();
    assert_eq!(count_call.cost_usd, Some(0.0));
}

#[tokio::test]
async fn budget_cuts_stream_and_blocks_further_calls() {
    // 200 chunks * 100 tokens = 20k output tokens = $0.20 at sonnet 5.5 rates.
    let upstream = mock_upstream(200).await;
    let (proxy, events, url) = start_proxy(&upstream).await;
    let run = Uuid::new_v4();
    let token = proxy.issue_token(run, Some(0.05), 0.0);
    let client = reqwest::Client::new();

    let body = client
        .post(format!("{url}/v1/messages"))
        .header("x-api-key", &token)
        .json(&message_request(true))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("budget_exceeded"), "stream was not cut");
    assert!(!body.contains("message_delta"));

    let calls = events.wait_for_calls(1).await;
    assert!(calls[0].cut_off);
    let charged = calls[0].cost_usd.unwrap();
    assert!((0.05..0.07).contains(&charged), "charged {charged}");

    for _ in 0..50 {
        if !events.exhausted.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(events.exhausted.lock().unwrap()[0].0, run);

    let res = client
        .post(format!("{url}/v1/messages"))
        .header("x-api-key", &token)
        .json(&message_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 402);
}

#[tokio::test]
async fn revoked_token_is_refused() {
    let upstream = mock_upstream(1).await;
    let (proxy, _events, url) = start_proxy(&upstream).await;
    let run = Uuid::new_v4();
    let token = proxy.issue_token(run, None, 0.0);
    proxy.revoke_run(run);
    let res = reqwest::Client::new()
        .post(format!("{url}/v1/messages"))
        .header("x-api-key", &token)
        .json(&message_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}
