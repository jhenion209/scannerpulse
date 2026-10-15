mod common;

use std::time::Duration;

use common::{Answer, FakeAsr};
use scannerpulse::asr::{AsrError, Client, Request};

fn request<'a>(url: &'a str, timeout: Duration) -> Request<'a> {
    Request {
        url,
        model: "model-x",
        language: Some("en"),
        prompt: Some("Engine 5, Main Street"),
        timeout,
    }
}

#[tokio::test]
async fn the_text_comes_back_trimmed_and_the_request_carries_every_field() {
    let server = FakeAsr::start(vec![Answer::Text("  Engine 5 responding.  ")]);
    let client = Client::new(Some("key-1".to_string()));
    let text = client
        .transcribe(
            &request(&server.url, Duration::from_secs(5)),
            "call.mp3",
            b"ID3audio".to_vec(),
        )
        .await;
    assert_eq!(text, Ok("Engine 5 responding.".to_string()));
    let sent = &server.requests()[0];
    assert!(sent.starts_with("POST /v1/audio/transcriptions "), "{sent}");
    assert!(sent.to_lowercase().contains("authorization: bearer key-1"));
    for part in [
        "name=\"model\"",
        "model-x",
        "name=\"language\"",
        "name=\"prompt\"",
        "Engine 5, Main Street",
        "filename=\"call.mp3\"",
        "ID3audio",
    ] {
        assert!(sent.contains(part), "{part} missing");
    }
}

#[tokio::test]
async fn no_key_no_language_and_no_prompt_are_left_out() {
    let server = FakeAsr::start(vec![Answer::Text("")]);
    let client = Client::new(None);
    let bare = Request {
        url: &server.url,
        model: "model-x",
        language: None,
        prompt: None,
        timeout: Duration::from_secs(5),
    };
    assert_eq!(
        client.transcribe(&bare, "c.mp3", vec![1]).await,
        Ok(String::new())
    );
    let sent = &server.requests()[0];
    assert!(!sent.to_lowercase().contains("authorization"));
    assert!(!sent.contains("name=\"language\""));
    assert!(!sent.contains("name=\"prompt\""));
}

#[tokio::test]
async fn a_4xx_is_the_call_s_fault() {
    let server = FakeAsr::start(vec![Answer::Status(400, "{\"error\": \"bad audio\"}")]);
    match Client::new(None)
        .transcribe(
            &request(&server.url, Duration::from_secs(5)),
            "c.mp3",
            vec![1],
        )
        .await
    {
        Err(AsrError::Call(why)) => {
            assert!(why.contains("400") && why.contains("bad audio"), "{why}")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_5xx_an_answer_without_text_a_refusal_and_silence_are_the_server_s_fault() {
    let busy = FakeAsr::start(vec![Answer::Status(503, "busy")]);
    let odd = FakeAsr::start(vec![Answer::Raw("{\"txt\": 1}")]);
    let silent = FakeAsr::start(vec![Answer::Silent]);
    let refused = common::refused_url();
    let client = Client::new(None);
    for (url, timeout) in [
        (busy.url.as_str(), 5),
        (odd.url.as_str(), 5),
        (refused.as_str(), 5),
        (silent.url.as_str(), 1),
    ] {
        let result = client
            .transcribe(
                &request(url, Duration::from_secs(timeout)),
                "c.mp3",
                vec![1],
            )
            .await;
        assert!(
            matches!(result, Err(AsrError::Server(_))),
            "{url}: {result:?}"
        );
    }
}

#[tokio::test]
async fn an_auth_path_or_rate_limit_refusal_is_the_server_s_fault() {
    for code in [401u16, 403, 404, 408, 429] {
        let server = FakeAsr::start(vec![Answer::Status(code, "refused")]);
        let result = Client::new(None)
            .transcribe(
                &request(&server.url, Duration::from_secs(5)),
                "c.mp3",
                vec![1],
            )
            .await;
        match result {
            Err(AsrError::Server(why)) => assert!(why.contains(&code.to_string()), "{why}"),
            other => panic!("{code}: {other:?}"),
        }
    }
    let server = FakeAsr::start(vec![Answer::Status(422, "unprocessable")]);
    let result = Client::new(None)
        .transcribe(
            &request(&server.url, Duration::from_secs(5)),
            "c.mp3",
            vec![1],
        )
        .await;
    assert!(matches!(result, Err(AsrError::Call(_))), "{result:?}");
}

#[tokio::test]
async fn a_nul_in_the_text_or_in_an_error_body_is_taken_out() {
    let server = FakeAsr::start(vec![
        Answer::Raw(r#"{"text": "Engine\u0000 5"}"#),
        Answer::Status(400, "bad\0 audio"),
    ]);
    let client = Client::new(None);
    let ask = request(&server.url, Duration::from_secs(5));
    assert_eq!(
        client.transcribe(&ask, "c.mp3", vec![1]).await,
        Ok("Engine 5".to_string())
    );
    match client.transcribe(&ask, "c.mp3", vec![1]).await {
        Err(AsrError::Call(why)) => {
            assert!(!why.contains('\0'), "{why:?}");
            assert!(why.contains("bad audio"), "{why:?}");
        }
        other => panic!("{other:?}"),
    }
}
