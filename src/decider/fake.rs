//! A scripted [`Decider`] for engine unit tests. No network, no clock.
//!
//! Queue replies with [`ScriptedDecider::push_answer`] or
//! [`ScriptedDecider::push_error`]; each `decide` call pops the next one
//! and records the request it was given. An empty queue answers with a
//! `connect` error, so a test that consults more than it scripted fails
//! visibly instead of hanging.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use super::types::{Answer, Decider, DeciderError, DecisionRequest, DecisionResponse, ErrorClass};

#[derive(Debug, Default)]
struct Script {
    replies: VecDeque<Result<DecisionResponse, DeciderError>>,
    requests: Vec<DecisionRequest>,
}

/// A [`Decider`] that returns queued replies in order.
#[derive(Debug)]
pub struct ScriptedDecider {
    provider: String,
    script: Mutex<Script>,
}

impl Default for ScriptedDecider {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedDecider {
    /// A fake whose `provider()` is `fake`.
    pub fn new() -> Self {
        Self::with_provider("fake")
    }

    pub fn with_provider(provider: &str) -> Self {
        ScriptedDecider {
            provider: provider.to_string(),
            script: Mutex::new(Script::default()),
        }
    }

    /// Queue a reply.
    pub fn push(&self, reply: Result<DecisionResponse, DeciderError>) -> &Self {
        self.lock().replies.push_back(reply);
        self
    }

    /// Queue a successful answer.
    pub fn push_answer(&self, response: DecisionResponse) -> &Self {
        self.push(Ok(response))
    }

    /// Queue an error.
    pub fn push_error(&self, error: DeciderError) -> &Self {
        self.push(Err(error))
    }

    /// How many times `decide` was called.
    pub fn calls(&self) -> usize {
        self.lock().requests.len()
    }

    /// Every request `decide` received, in order.
    pub fn requests(&self) -> Vec<DecisionRequest> {
        self.lock().requests.clone()
    }

    /// Replies still queued.
    pub fn remaining(&self) -> usize {
        self.lock().replies.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Script> {
        self.script.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Decider for ScriptedDecider {
    fn provider(&self) -> &str {
        &self.provider
    }

    fn decide(&self, req: &DecisionRequest) -> Result<DecisionResponse, DeciderError> {
        let mut script = self.lock();
        script.requests.push(req.clone());
        script.replies.pop_front().unwrap_or_else(|| {
            Err(DeciderError::new(
                ErrorClass::Connect,
                "scripted decider has no reply queued",
            ))
        })
    }
}

/// Build a response from `(field, answer)` pairs, with model `model`.
pub fn response(model: &str, answers: Vec<(&str, Answer)>) -> DecisionResponse {
    DecisionResponse {
        model: model.to_string(),
        answers: answers
            .into_iter()
            .map(|(k, a)| (k.to_string(), a))
            .collect(),
    }
}

/// An enum answer from `(value, probability)` pairs.
pub fn choice(pairs: &[(&str, f64)]) -> Answer {
    Answer::Choice {
        probabilities: pairs
            .iter()
            .map(|(k, p)| (k.to_string(), *p))
            .collect::<BTreeMap<_, _>>(),
        provider_confidence: None,
    }
}

/// A boolean answer with P(true) = `p_true`.
pub fn proposition(p_true: f64) -> Answer {
    Answer::Proposition { p_true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> DecisionRequest {
        DecisionRequest {
            questions: vec![],
            inputs: vec![],
        }
    }

    #[test]
    fn replays_in_order_and_records() {
        let d = ScriptedDecider::new();
        d.push_answer(response("m1", vec![("ready", proposition(0.9))]))
            .push_error(DeciderError::timeout());
        let boxed: &dyn Decider = &d;
        assert_eq!(boxed.provider(), "fake");
        assert_eq!(boxed.decide(&req()).unwrap().model, "m1");
        assert_eq!(boxed.decide(&req()).unwrap_err().class, ErrorClass::Timeout);
        assert_eq!(boxed.decide(&req()).unwrap_err().class, ErrorClass::Connect);
        assert_eq!(d.calls(), 3);
        assert_eq!(d.remaining(), 0);
        assert_eq!(d.requests().len(), 3);
    }

    #[test]
    fn helpers_build_answers() {
        match choice(&[("a", 0.6), ("b", 0.4)]) {
            Answer::Choice { probabilities, .. } => assert_eq!(probabilities["a"], 0.6),
            other => panic!("{:?}", other),
        }
    }
}
