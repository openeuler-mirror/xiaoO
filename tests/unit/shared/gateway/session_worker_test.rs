use super::*;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Test handle whose `ask` parks until a response is delivered through an
/// internal channel (or the channel closes), mirroring handles that block
/// on external user input (SSE interaction store, TUI prompt channel).
#[derive(Default)]
struct ParkingInteractionHandle {
    abort_pending_calls: AtomicUsize,
    answered: AtomicBool,
}

impl ParkingInteractionHandle {
    fn ask_signal(&self) -> &AtomicBool {
        &self.answered
    }
}

#[async_trait]
impl InteractionHandle for ParkingInteractionHandle {
    async fn ask(&self, _request: &InteractionRequest) -> InteractionResponse {
        // Park until the test flips `answered`; a buggy wrapper that
        // neither cancels nor forwards would hang the test (tokio test
        // timeout).
        while !self.answered.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        InteractionResponse::Confirmed { allowed: true }
    }

    async fn abort_pending(&self, _request: &InteractionRequest) {
        self.abort_pending_calls.fetch_add(1, Ordering::SeqCst);
    }
}

fn choice_request() -> InteractionRequest {
    InteractionRequest::Choice {
        prompt: "allow?".to_string(),
        options: vec!["Allow".to_string(), "Deny".to_string()],
        allow_custom_input: false,
        source: None,
    }
}

#[tokio::test]
async fn cancel_unparks_pending_ask_with_deny_response_and_aborts() {
    let inner = Arc::new(ParkingInteractionHandle::default());
    let token = CancellationToken::new();
    let wrapper = Arc::new(CancelAwareInteractionHandle::new(
        Arc::clone(&inner) as Arc<dyn InteractionHandle>,
        token.clone(),
    ));

    let ask = {
        let wrapper = Arc::clone(&wrapper);
        let request = choice_request();
        tokio::spawn(async move { wrapper.ask(&request).await })
    };
    // Give the ask a chance to park.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    token.cancel();
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), ask)
        .await
        .expect("ask must unpark after cancel")
        .expect("ask task must not panic");

    assert!(
        matches!(response, InteractionResponse::Choice { value: None }),
        "cancelled ask must resolve deny-style, got {response:?}"
    );
    assert_eq!(
        inner.abort_pending_calls.load(Ordering::SeqCst),
        1,
        "wrapper must release the inner pending entry on cancel"
    );
}

#[tokio::test]
async fn pre_cancelled_token_short_circuits_ask() {
    let inner = Arc::new(ParkingInteractionHandle::default());
    let token = CancellationToken::new();
    token.cancel();
    let wrapper =
        CancelAwareInteractionHandle::new(Arc::clone(&inner) as Arc<dyn InteractionHandle>, token);

    let request = choice_request();
    let response = wrapper.ask(&request).await;
    assert!(matches!(
        response,
        InteractionResponse::Choice { value: None }
    ));
    assert_eq!(inner.abort_pending_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn inner_response_passes_through_when_not_cancelled() {
    let inner = Arc::new(ParkingInteractionHandle::default());
    let wrapper = Arc::new(CancelAwareInteractionHandle::new(
        Arc::clone(&inner) as Arc<dyn InteractionHandle>,
        CancellationToken::new(),
    ));

    let ask = {
        let wrapper = Arc::clone(&wrapper);
        let request = choice_request();
        tokio::spawn(async move { wrapper.ask(&request).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    // The inner handle answers (allowed=true) without any cancellation.
    inner.ask_signal().store(true, Ordering::SeqCst);

    let response = tokio::time::timeout(std::time::Duration::from_secs(2), ask)
        .await
        .expect("ask must complete when the inner handle answers")
        .expect("ask task must not panic");
    assert!(matches!(
        response,
        InteractionResponse::Confirmed { allowed: true }
    ));
    assert_eq!(inner.abort_pending_calls.load(Ordering::SeqCst), 0);
}
