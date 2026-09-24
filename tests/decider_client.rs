//! The decider client against the `std::net` stub: `build_decider` and
//! its opt-in gate, the Jev wire format and response validation, and the
//! bounded transport.
//!
//! Every test that reaches the stub resolves its settings in-process from
//! an explicit user config and an explicit env map that names
//! `KOTO_DECIDER`, `KOTO_DECIDER_API_KEY`, and `KOTO_DECIDER_ENDPOINT`, so
//! nothing depends on a developer's exported environment. No test talks to
//! a real provider.

#[path = "support/decider_stub.rs"]
mod decider_stub;

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use decider_stub::{closed_url, DeciderStub, Reply};
use koto::config::resolve::{
    apply_decider_env, merge_decider, resolve_decider, ConfigLayer, DeciderSettings,
};
use koto::config::{DeciderConfig, KotoConfig};
use koto::decider::http::{post_json_with_deadline, MAX_RESPONSE_BYTES};
use koto::decider::{
    build_decider, build_request, declared_fields, AnswerOption, ApiKey, DeciderError,
    DecisionRequest, ErrorClass, LabelledInput, Question, QuestionKind,
};
use koto::template::decider::{
    DeciderAnswer, DeciderEscape, DeciderInput, DeciderInputSource, DeciderMode, FieldDecider,
};
use koto::template::types::FieldSchema;
use serde_json::{json, Value};
use url::Url;

const KEY: &str = "sk-koto-stub-DISTINCTIVE-9e1f4c";
const MARKER: &str = "BODY-MARKER-5d2b8a";

// ---------------------------------------------------------------------------
// Settings, resolved the way load_config does
// ---------------------------------------------------------------------------

fn resolve(user: &str, project: &str, env: &[(&str, &str)]) -> DeciderSettings {
    let parse = |body: &str| -> DeciderConfig {
        let cfg: KotoConfig = toml::from_str(body).expect("toml");
        cfg.decider
    };
    let mut cfg = DeciderConfig::default();
    merge_decider(&mut cfg, &parse(user), ConfigLayer::User);
    merge_decider(&mut cfg, &parse(project), ConfigLayer::Project);
    let map: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    apply_decider_env(&mut cfg, |k| map.get(k).cloned());
    resolve_decider(&cfg).0
}

/// Env-layer settings: mode, key, and endpoint all from the env map.
fn env_settings(mode: &str, endpoint: &str, timeout_ms: Option<u64>) -> DeciderSettings {
    let user = match timeout_ms {
        Some(ms) => format!("[decider]\ntimeout_ms = {}\n", ms),
        None => String::new(),
    };
    resolve(
        &user,
        "",
        &[
            ("KOTO_DECIDER", mode),
            ("KOTO_DECIDER_API_KEY", KEY),
            ("KOTO_DECIDER_ENDPOINT", endpoint),
        ],
    )
}

fn client(stub: &DeciderStub) -> Box<dyn koto::decider::Decider> {
    build_decider(&env_settings("shadow", &stub.url(), None)).expect("opted in")
}

// ---------------------------------------------------------------------------
// A declaration: enum `verdict` and boolean `ready`
// ---------------------------------------------------------------------------

fn answer(description: &str) -> DeciderAnswer {
    DeciderAnswer {
        description: description.to_string(),
        mode: DeciderMode::Shadow,
        threshold: 0.9,
    }
}

fn input(label: &str) -> DeciderInput {
    DeciderInput {
        source: DeciderInputSource::Context(format!("{}.md", label)),
        label: label.to_string(),
        max_bytes: 8192,
    }
}

fn accepts() -> BTreeMap<String, FieldSchema> {
    let mut enum_answers = BTreeMap::new();
    enum_answers.insert("proceed".to_string(), answer("Clear and scoped."));
    enum_answers.insert("exit".to_string(), answer("Vague."));
    let mut bool_answers = BTreeMap::new();
    bool_answers.insert("true".to_string(), answer("Ready."));
    bool_answers.insert("false".to_string(), answer("Not ready."));

    let mut a = BTreeMap::new();
    a.insert(
        "verdict".to_string(),
        FieldSchema {
            field_type: "enum".to_string(),
            required: true,
            values: vec!["proceed".to_string(), "exit".to_string()],
            description: "Is the item clear enough?".to_string(),
            decider: Some(FieldDecider {
                answers: enum_answers,
                escape: Some(DeciderEscape {
                    value: "unclear".to_string(),
                    description: "Can't tell from the inputs.".to_string(),
                }),
                inputs: vec![input("outline_item"), input("plan")],
            }),
        },
    );
    a.insert(
        "ready".to_string(),
        FieldSchema {
            field_type: "boolean".to_string(),
            required: true,
            values: vec![],
            description: "The change is ready to merge.".to_string(),
            decider: Some(FieldDecider {
                answers: bool_answers,
                escape: None,
                inputs: vec![input("plan"), input("diff_stat")],
            }),
        },
    );
    a
}

fn request() -> DecisionRequest {
    let accepts = accepts();
    let fields = declared_fields(&accepts);
    let inputs: BTreeMap<String, String> = [
        ("outline_item", "Add the trait."),
        ("plan", "The plan."),
        ("diff_stat", "2 files changed"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    build_request(&fields, &inputs).expect("request")
}

fn good_answer() -> Value {
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "ready": {"type": "noul", "noul": 0.93},
            "verdict": {"type": "choice", "choice": "proceed",
                        "probabilities": {"proceed": 0.8, "exit": 0.15, "unclear": 0.05},
                        "confidence": 0.61}
        },
        "usage": {"input_tokens": 120, "output_tokens": 4}
    })
}

fn decide_with(reply: Reply) -> (Result<koto::decider::DecisionResponse, DeciderError>, usize) {
    let stub = DeciderStub::start();
    stub.push(reply);
    let got = client(&stub).decide(&request());
    (got, stub.request_count())
}

fn class_of(reply: Reply) -> ErrorClass {
    let (got, _) = decide_with(reply);
    got.expect_err("expected an error").class
}

// ---------------------------------------------------------------------------
// build_decider and the opt-in gate
// ---------------------------------------------------------------------------

#[test]
fn user_key_with_env_only_endpoint_builds_nothing_and_sends_nothing() {
    let stub = DeciderStub::start();
    let settings = resolve(
        &format!("[decider]\nmode = \"shadow\"\napi_key = \"{}\"\n", KEY),
        "",
        &[
            ("KOTO_DECIDER", "shadow"),
            ("KOTO_DECIDER_API_KEY", ""),
            ("KOTO_DECIDER_ENDPOINT", &stub.url()),
        ],
    );
    assert!(!settings.opted_in());
    assert!(build_decider(&settings).is_none());
    assert_eq!(stub.request_count(), 0);
}

#[test]
fn project_off_lowers_a_user_auto_to_nothing() {
    let settings = resolve(
        &format!("[decider]\nmode = \"auto\"\napi_key = \"{}\"\n", KEY),
        "[decider]\nmode = \"off\"\n",
        &[
            ("KOTO_DECIDER", ""),
            ("KOTO_DECIDER_API_KEY", ""),
            ("KOTO_DECIDER_ENDPOINT", ""),
        ],
    );
    assert!(build_decider(&settings).is_none());
}

#[test]
fn same_layer_settings_build_a_jev_client() {
    let stub = DeciderStub::start();
    for mode in ["shadow", "auto"] {
        let env = env_settings(mode, &stub.url(), None);
        assert_eq!(build_decider(&env).expect(mode).provider(), "jev");

        let user = resolve(
            &format!(
                "[decider]\nmode = \"{}\"\napi_key = \"{}\"\nendpoint = \"{}\"\n",
                mode,
                KEY,
                stub.url()
            ),
            "",
            &[
                ("KOTO_DECIDER", ""),
                ("KOTO_DECIDER_API_KEY", ""),
                ("KOTO_DECIDER_ENDPOINT", ""),
            ],
        );
        assert_eq!(build_decider(&user).expect(mode).provider(), "jev");
    }
    assert_eq!(stub.request_count(), 0, "building sends nothing");
}

#[test]
fn client_keeps_the_endpoint_it_was_built_with() {
    let first = DeciderStub::start();
    let second = DeciderStub::start();
    first.push(Reply::json(&good_answer()));
    second.push(Reply::json(&good_answer()));

    let decider = client(&first);
    let previous = std::env::var("KOTO_DECIDER_ENDPOINT").ok();
    std::env::set_var("KOTO_DECIDER_ENDPOINT", second.url());
    let got = decider.decide(&request());
    match previous {
        Some(v) => std::env::set_var("KOTO_DECIDER_ENDPOINT", v),
        None => std::env::remove_var("KOTO_DECIDER_ENDPOINT"),
    }

    got.expect("answer");
    assert_eq!(first.request_count(), 1);
    assert_eq!(second.request_count(), 0);
}

#[test]
fn client_uses_the_configured_timeout() {
    let stub = DeciderStub::start();
    stub.push(Reply::json(&good_answer()).delay(Duration::from_secs(10)));
    let decider = build_decider(&env_settings("shadow", &stub.url(), Some(300))).unwrap();
    let start = Instant::now();
    let err = decider.decide(&request()).unwrap_err();
    assert_eq!(err.class, ErrorClass::Timeout);
    assert!(start.elapsed() < Duration::from_millis(300 + 250));
}

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

#[test]
fn request_body_has_exactly_the_jev_shape() {
    let stub = DeciderStub::start();
    stub.push(Reply::json(&good_answer()));
    client(&stub).decide(&request()).expect("answer");
    let rec = stub.last_request().unwrap();
    assert_eq!(rec.method, "POST");
    assert_eq!(rec.path, decider_stub::STUB_PATH);

    let body = rec.body_str();
    // Exact bytes: fixed key order, declaration order, nothing else.
    assert_eq!(
        body,
        concat!(
            r#"{"model":"jev-latest","#,
            r#""state":{"plan":"The plan.","diff_stat":"2 files changed","outline_item":"Add the trait."},"#,
            r#""questions":{"#,
            r#""ready":{"type":"noul","instructions":"The change is ready to merge."},"#,
            r#""verdict":{"type":"choice","instructions":"Is the item clear enough?","#,
            r#""criteria":{"proceed":"Clear and scoped.","exit":"Vague.","unclear":"Can't tell from the inputs."}}"#,
            r#"}}"#
        )
    );

    let v = rec.json();
    let top: Vec<&String> = v.as_object().unwrap().keys().collect();
    let mut expected = vec!["model", "questions", "state"];
    expected.sort();
    let mut top_sorted: Vec<&str> = top.iter().map(|s| s.as_str()).collect();
    top_sorted.sort();
    assert_eq!(top_sorted, expected);
    assert_eq!(v["model"], "jev-latest");
    assert_eq!(
        v["questions"]["ready"].as_object().unwrap().len(),
        2,
        "noul carries type and instructions only"
    );
    assert_eq!(v["questions"]["verdict"].as_object().unwrap().len(), 3);
}

#[test]
fn same_request_serializes_byte_identically() {
    let stub = DeciderStub::start();
    stub.push(Reply::json(&good_answer()));
    stub.push(Reply::json(&good_answer()));
    let d = client(&stub);
    d.decide(&request()).unwrap();
    d.decide(&request()).unwrap();
    let reqs = stub.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].body, reqs[1].body);
}

#[test]
fn request_headers() {
    let stub = DeciderStub::start();
    stub.push(Reply::json(&good_answer()));
    client(&stub).decide(&request()).unwrap();
    let rec = stub.last_request().unwrap();
    assert_eq!(
        rec.header("authorization"),
        Some(format!("Bearer {}", KEY).as_str())
    );
    assert_eq!(rec.header("content-type"), Some("application/json"));
    assert_eq!(
        rec.header("user-agent"),
        Some(format!("koto/{}", env!("CARGO_PKG_VERSION")).as_str())
    );
}

#[test]
fn valid_answer_decodes() {
    let (got, count) = decide_with(Reply::json(&good_answer()));
    let r = got.expect("answer");
    assert_eq!(count, 1);
    assert_eq!(r.model, "jev-1.13.0");
    assert_eq!(r.answers.len(), 2);
}

// ---------------------------------------------------------------------------
// Response validation
// ---------------------------------------------------------------------------

fn with_answer(field: &str, answer: Value) -> Value {
    let mut v = good_answer();
    v["answers"][field] = answer;
    v
}

#[test]
fn http_errors_are_http_status() {
    for status in [500u16, 401, 403, 422, 429, 529] {
        let (got, _) = decide_with(Reply::raw(status, MARKER));
        let err = got.unwrap_err();
        assert_eq!(err.class, ErrorClass::HttpStatus, "{}", status);
        assert_eq!(err.status, Some(status));
    }
}

#[test]
fn non_json_body_is_malformed() {
    assert_eq!(
        class_of(Reply::raw(200, format!("<html>{}</html>", MARKER))),
        ErrorClass::Malformed
    );
}

#[test]
fn missing_answers_is_malformed() {
    assert_eq!(
        class_of(Reply::json(&json!({"model": "jev-1", "note": MARKER}))),
        ErrorClass::Malformed
    );
}

#[test]
fn missing_answer_for_an_asked_field_is_mismatched() {
    let mut v = good_answer();
    v["answers"].as_object_mut().unwrap().remove("ready");
    assert_eq!(class_of(Reply::json(&v)), ErrorClass::Mismatched);
}

#[test]
fn answer_for_an_unasked_field_is_mismatched() {
    let v = with_answer("bogus", json!({"type": "noul", "noul": 0.5}));
    assert_eq!(class_of(Reply::json(&v)), ErrorClass::Mismatched);
}

#[test]
fn undeclared_choice_key_is_mismatched() {
    let v = with_answer(
        "verdict",
        json!({"type": "choice", "probabilities": {"proceed": 0.8, "merge": 0.15, "unclear": 0.05}}),
    );
    assert_eq!(class_of(Reply::json(&v)), ErrorClass::Mismatched);
}

#[test]
fn missing_choice_key_is_mismatched() {
    let v = with_answer(
        "verdict",
        json!({"type": "choice", "probabilities": {"proceed": 0.95, "unclear": 0.05}}),
    );
    assert_eq!(class_of(Reply::json(&v)), ErrorClass::Mismatched);
}

#[test]
fn nan_negative_and_above_one_are_malformed() {
    // JSON has no NaN literal; a provider sending one sends invalid JSON.
    let nan = good_answer()
        .to_string()
        .replace("\"proceed\":0.8", "\"proceed\":NaN");
    assert!(nan.contains("NaN"));
    assert_eq!(class_of(Reply::raw(200, nan)), ErrorClass::Malformed);

    let neg = with_answer(
        "verdict",
        json!({"type": "choice", "probabilities": {"proceed": 1.1, "exit": -0.15, "unclear": 0.05}}),
    );
    assert_eq!(class_of(Reply::json(&neg)), ErrorClass::Malformed);

    let above = with_answer(
        "verdict",
        json!({"type": "choice", "probabilities": {"proceed": 1.5, "exit": 0.0, "unclear": 0.0}}),
    );
    assert_eq!(class_of(Reply::json(&above)), ErrorClass::Malformed);
}

#[test]
fn choice_sum_tolerance_is_one_hundredth() {
    let off = with_answer(
        "verdict",
        json!({"type": "choice", "probabilities": {"proceed": 0.82, "exit": 0.15, "unclear": 0.05}}),
    );
    assert_eq!(class_of(Reply::json(&off)), ErrorClass::Malformed);

    let near = with_answer(
        "verdict",
        json!({"type": "choice", "probabilities": {"proceed": 0.809, "exit": 0.15, "unclear": 0.05}}),
    );
    let (got, _) = decide_with(Reply::json(&near));
    got.expect("1.009 is within tolerance");
}

#[test]
fn noul_outside_unit_interval_is_malformed() {
    for p in [1.2, -0.1] {
        let v = with_answer("ready", json!({"type": "noul", "noul": p}));
        assert_eq!(class_of(Reply::json(&v)), ErrorClass::Malformed);
    }
}

#[test]
fn type_that_does_not_match_the_question_is_malformed() {
    let v = with_answer("ready", json!({"type": "score", "score": 0.5}));
    assert_eq!(class_of(Reply::json(&v)), ErrorClass::Malformed);
    let v = with_answer("verdict", json!({"type": "noul", "noul": 0.5}));
    assert_eq!(class_of(Reply::json(&v)), ErrorClass::Malformed);
}

#[test]
fn choice_without_probabilities_is_malformed() {
    let v = with_answer(
        "verdict",
        json!({"type": "choice", "choice": "proceed", "confidence": 0.9}),
    );
    assert_eq!(class_of(Reply::json(&v)), ErrorClass::Malformed);
}

// ---------------------------------------------------------------------------
// Model handling
// ---------------------------------------------------------------------------

fn model_of(model: Option<Value>) -> String {
    let mut v = good_answer();
    match model {
        Some(m) => v["model"] = m,
        None => {
            v.as_object_mut().unwrap().remove("model");
        }
    }
    let (got, _) = decide_with(Reply::json(&v));
    got.expect("model problems never fail the answer").model
}

#[test]
fn missing_empty_or_non_string_model_is_unknown() {
    assert_eq!(model_of(None), "unknown");
    assert_eq!(model_of(Some(json!(""))), "unknown");
    assert_eq!(model_of(Some(json!(12))), "unknown");
    assert_eq!(model_of(Some(json!(null))), "unknown");
}

#[test]
fn model_is_trimmed_stripped_and_capped() {
    let raw = format!("  \u{1b}[2Jjev\u{0}-{}\t\n", "y".repeat(300));
    let got = model_of(Some(json!(raw)));
    assert!(!got.chars().any(|c| c.is_control()), "{:?}", got);
    assert!(got.starts_with("[2Jjev-yyy"), "{:?}", got);
    assert_eq!(got.chars().count(), 128);
    assert_eq!(got, got.trim());
}

// ---------------------------------------------------------------------------
// Refused before sending
// ---------------------------------------------------------------------------

fn opt(v: &str) -> AnswerOption {
    AnswerOption {
        value: v.to_string(),
        description: format!("{} desc", v),
    }
}

#[test]
fn too_many_choice_keys_is_malformed_without_sending() {
    let stub = DeciderStub::start();
    let req = DecisionRequest {
        questions: vec![Question {
            field: "verdict".to_string(),
            kind: QuestionKind::Choice {
                question: "Q?".to_string(),
                options: (0..255).map(|i| opt(&format!("v{}", i))).collect(),
                escape: opt("unclear"),
            },
        }],
        inputs: vec![],
    };
    let err = client(&stub).decide(&req).unwrap_err();
    assert_eq!(err.class, ErrorClass::Malformed);
    assert_eq!(stub.request_count(), 0);
}

#[test]
fn too_few_choice_keys_is_malformed_without_sending() {
    let stub = DeciderStub::start();
    let req = DecisionRequest {
        questions: vec![Question {
            field: "verdict".to_string(),
            kind: QuestionKind::Choice {
                question: "Q?".to_string(),
                options: vec![],
                escape: opt("unclear"),
            },
        }],
        inputs: vec![],
    };
    assert_eq!(
        client(&stub).decide(&req).unwrap_err().class,
        ErrorClass::Malformed
    );
    assert_eq!(stub.request_count(), 0);
}

#[test]
fn duplicate_input_labels_are_malformed_without_sending() {
    let stub = DeciderStub::start();
    let mut req = request();
    req.inputs.push(LabelledInput {
        label: "plan".to_string(),
        content: "again".to_string(),
    });
    let err = client(&stub).decide(&req).unwrap_err();
    assert_eq!(err.class, ErrorClass::Malformed);
    assert_eq!(stub.request_count(), 0);
}

// ---------------------------------------------------------------------------
// Secrets never reach errors or stderr
// ---------------------------------------------------------------------------

fn assert_clean(err: &DeciderError) {
    for text in [format!("{:?}", err), err.to_string()] {
        assert!(!text.contains(KEY), "key leaked: {}", text);
        assert!(!text.contains(MARKER), "body leaked: {}", text);
        assert!(
            !text.contains("userinfo-secret"),
            "userinfo leaked: {}",
            text
        );
    }
    assert!(err.detail.chars().count() <= 200);
}

#[test]
fn errors_carry_no_key_body_or_userinfo() {
    let replies = vec![
        Reply::raw(401, format!("{{\"error\":\"{} {}\"}}", MARKER, KEY)),
        Reply::raw(500, MARKER),
        Reply::raw(302, MARKER).header("Location", &format!("http://{}.example/", MARKER)),
        Reply::raw(200, format!("not json {}", MARKER)),
        Reply::json(&json!({"model": MARKER, "answers": {MARKER: {"type": "noul", "noul": 0.5}}})),
        Reply::json(&with_answer(
            "verdict",
            json!({"type": "choice", "probabilities": {MARKER: 1.0}}),
        )),
    ];
    for reply in replies {
        let (got, _) = decide_with(reply);
        assert_clean(&got.unwrap_err());
    }
    // An endpoint with userinfo is refused before any client exists.
    let stub = DeciderStub::start();
    let with_userinfo = format!(
        "http://user:userinfo-secret@127.0.0.1:{}{}",
        stub.port(),
        decider_stub::STUB_PATH
    );
    assert!(build_decider(&env_settings("shadow", &with_userinfo, None)).is_none());
    assert_eq!(stub.request_count(), 0);
    // Transport failures too.
    let err = build_decider(&env_settings("shadow", &closed_url(), None))
        .unwrap()
        .decide(&request())
        .unwrap_err();
    assert_clean(&err);
}

/// Runs only as a child of `http_401_prints_one_fixed_stderr_line`.
#[test]
fn stderr_child_401() {
    if std::env::var("KOTO_DECIDER_TEST_STDERR_CHILD").is_err() {
        return;
    }
    let stub = DeciderStub::start();
    stub.push(Reply::raw(
        401,
        format!("{{\"error\":\"bad key {} {}\"}}", KEY, MARKER),
    ));
    let err = client(&stub).decide(&request()).unwrap_err();
    assert_eq!(err.class, ErrorClass::HttpStatus);
    assert_eq!(err.status, Some(401));
}

#[test]
fn http_401_prints_one_fixed_stderr_line() {
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args([
            "--exact",
            "stderr_child_401",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("KOTO_DECIDER_TEST_STDERR_CHILD", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "child failed: {:?}", out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    for text in [&stderr, &stdout] {
        assert!(!text.contains(KEY), "key leaked: {}", text);
        assert!(!text.contains(MARKER), "body leaked: {}", text);
    }
    let lines: Vec<&str> = stderr.lines().filter(|l| l.contains("decider")).collect();
    assert_eq!(lines.len(), 1, "stderr: {}", stderr);
    assert!(lines[0].contains("HTTP 401"), "{}", lines[0]);
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// Raw transport call. The endpoint and key still come from settings
/// resolved from an explicit env map, like every other stub test.
fn post(url: &str, budget: Duration) -> Result<(u16, Vec<u8>), DeciderError> {
    let settings = env_settings("shadow", url, None);
    let endpoint: &Url = settings.endpoint().expect("loopback endpoint accepted");
    let key: &ApiKey = settings.api_key().expect("key");
    post_json_with_deadline(endpoint, key, b"{}".to_vec(), budget)
}

#[test]
fn slow_response_times_out_within_budget() {
    let stub = DeciderStub::start();
    stub.push(Reply::json(&good_answer()).delay(Duration::from_secs(10)));
    let start = Instant::now();
    let err = post(&stub.url(), Duration::from_millis(200)).unwrap_err();
    let took = start.elapsed();
    assert_eq!(err.class, ErrorClass::Timeout);
    assert!(took < Duration::from_millis(450), "took {:?}", took);
}

#[test]
fn held_request_times_out_within_budget() {
    let stub = DeciderStub::start();
    stub.push(Reply::json(&good_answer()).hold());
    let start = Instant::now();
    let err = post(&stub.url(), Duration::from_millis(200)).unwrap_err();
    assert_eq!(err.class, ErrorClass::Timeout);
    assert!(start.elapsed() < Duration::from_millis(450));
    assert!(stub.wait_for_requests(1, Duration::from_secs(1)));
    stub.release();
}

#[test]
fn closed_port_is_connect() {
    let err = post(&closed_url(), Duration::from_secs(2)).unwrap_err();
    assert_eq!(err.class, ErrorClass::Connect);
}

#[test]
fn redirects_are_not_followed() {
    let first = DeciderStub::start();
    let second = DeciderStub::start();
    first.push(Reply::status(302).header("Location", &second.url()));
    second.set_default(Reply::json(&good_answer()));

    let (status, _) = post(&first.url(), Duration::from_secs(2)).unwrap();
    assert_eq!(status, 302);

    first.push(Reply::status(302).header("Location", &second.url()));
    let err = client(&first).decide(&request()).unwrap_err();
    assert_eq!(err.class, ErrorClass::HttpStatus);
    assert_eq!(err.status, Some(302));
    assert_eq!(second.request_count(), 0);
}

#[test]
fn body_is_capped_at_one_mebibyte() {
    let stub = DeciderStub::start();
    stub.push(Reply::raw(200, vec![b'a'; MAX_RESPONSE_BYTES]));
    let (status, body) = post(&stub.url(), Duration::from_secs(5)).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body.len(), MAX_RESPONSE_BYTES);

    stub.push(Reply::raw(200, vec![b'a'; MAX_RESPONSE_BYTES + 1]));
    let err = post(&stub.url(), Duration::from_secs(5)).unwrap_err();
    assert_eq!(err.class, ErrorClass::Malformed);
}

#[test]
fn stub_records_and_counts() {
    let stub = DeciderStub::start();
    stub.push(Reply::status(204).header("X-Test", "1"));
    let (status, body) = post(&stub.url(), Duration::from_secs(2)).unwrap();
    assert_eq!(status, 204);
    assert!(body.is_empty());
    assert_eq!(stub.request_count(), 1);
    assert_eq!(stub.last_request().unwrap().body, b"{}");
    // The default reply is a 500.
    let (status, _) = post(&stub.url(), Duration::from_secs(2)).unwrap();
    assert_eq!(status, 500);
}
