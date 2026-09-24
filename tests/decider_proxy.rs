//! Proxy environment variables are ignored for loopback decider endpoints.
//!
//! This is its own test binary because it sets `HTTP_PROXY`, `HTTPS_PROXY`,
//! and `ALL_PROXY` for the whole process; keeping it apart means that
//! mutation can't race any other test.

#[path = "support/decider_stub.rs"]
mod decider_stub;

use std::collections::HashMap;
use std::time::Duration;

use decider_stub::{closed_port, DeciderStub, Reply};
use koto::config::resolve::{apply_decider_env, resolve_decider};
use koto::config::DeciderConfig;
use koto::decider::http::post_json_with_deadline;
use koto::decider::{build_decider, DecisionRequest, LabelledInput, Question, QuestionKind};
use serde_json::json;

const KEY: &str = "sk-koto-proxy-DISTINCTIVE-31a7";

fn request() -> DecisionRequest {
    DecisionRequest {
        questions: vec![Question {
            field: "ready".to_string(),
            kind: QuestionKind::Proposition {
                proposition: "Ready.".to_string(),
            },
        }],
        inputs: vec![LabelledInput {
            label: "plan".to_string(),
            content: "p".to_string(),
        }],
    }
}

#[test]
fn loopback_requests_bypass_proxy_env() {
    let dead_proxy = format!("http://127.0.0.1:{}", closed_port());
    for var in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        std::env::set_var(var, &dead_proxy);
    }
    std::env::remove_var("NO_PROXY");
    std::env::remove_var("no_proxy");

    let stub = DeciderStub::start();
    let answer = json!({"model": "jev-1", "answers": {"ready": {"type": "noul", "noul": 0.4}}});
    stub.set_default(Reply::json(&answer));

    for url in [stub.url(), stub.localhost_url()] {
        // Settings resolved from an explicit env map, not the process env.
        let env: HashMap<&str, String> = [
            ("KOTO_DECIDER", "shadow".to_string()),
            ("KOTO_DECIDER_API_KEY", KEY.to_string()),
            ("KOTO_DECIDER_ENDPOINT", url.clone()),
        ]
        .into_iter()
        .collect();
        let mut cfg = DeciderConfig::default();
        apply_decider_env(&mut cfg, |k| env.get(k).cloned());
        let settings = resolve_decider(&cfg).0;

        // The raw transport.
        let (status, _) = post_json_with_deadline(
            settings.endpoint().expect("endpoint"),
            settings.api_key().expect("key"),
            b"{}".to_vec(),
            Duration::from_secs(5),
        )
        .unwrap_or_else(|e| panic!("{}: {}", url, e));
        assert_eq!(status, 200, "{}", url);

        // And through a client built from the same settings.
        let decider = build_decider(&settings).expect("opted in");
        let got = decider
            .decide(&request())
            .unwrap_or_else(|e| panic!("{}: {}", url, e));
        assert_eq!(got.model, "jev-1");
    }
    assert_eq!(stub.request_count(), 4);
}
