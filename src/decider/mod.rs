//! Opt-in decider support.
//!
//! A decider is a typed decision model that koto can consult for a
//! template-declared decision instead of stopping for agent evidence.
//! Whether a user is opted in is decided in exactly one place:
//! [`crate::config::resolve::DeciderSettings::opted_in`], built by
//! [`crate::config::resolve::resolve_decider`].
//!
//! Layout:
//!
//! - `types`, `request`, `evaluate`, `record`: pure and provider-neutral. The engine
//!   depends only on these.
//! - `jev`, `http`: the Jev client and its bounded transport.
//! - [`build_decider`]: the only production constructor of a provider.

pub mod evaluate;
#[cfg(test)]
pub mod fake;
pub mod http;
pub mod jev;
pub mod record;
pub mod request;
pub mod types;

pub use evaluate::{evaluate, EffectiveModes, Evaluation, FieldEvaluation, FieldOutcome};
pub use record::{ConsultationOutcome, DeciderConsultation, FieldConsultation};
pub use request::{build_request, declared_fields, BuildRequestError, DeclaredField, DeclaredKind};
pub use types::{
    Answer, AnswerOption, ApiKey, Decider, DeciderError, DecisionRequest, DecisionResponse,
    ErrorClass, GlobalMode, LabelledInput, Question, QuestionKind, SettingOrigin,
};

use crate::config::resolve::DeciderSettings;

/// Build the configured provider, or `None` when the user isn't opted in.
///
/// The gate is [`DeciderSettings::opted_in`] and nothing else: no mode,
/// key, or endpoint rule is re-derived here. The client captures the
/// endpoint, key, and timeout from `settings` now and reads nothing from
/// the environment later.
pub fn build_decider(settings: &DeciderSettings) -> Option<Box<dyn Decider>> {
    if !settings.opted_in() {
        return None;
    }
    // Both are guaranteed present once opted_in() holds.
    let endpoint = settings.endpoint()?.clone();
    let key = settings.api_key()?.clone();
    Some(Box::new(jev::JevDecider::new(
        endpoint,
        key,
        settings.timeout(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::resolve::{apply_decider_env, merge_decider, resolve_decider, ConfigLayer};
    use crate::config::DeciderConfig;

    fn settings(user: &str, project: &str, env: &[(&str, &str)]) -> DeciderSettings {
        let parse = |body: &str| -> DeciderConfig {
            let cfg: crate::config::KotoConfig = toml::from_str(body).unwrap();
            cfg.decider
        };
        let mut cfg = DeciderConfig::default();
        merge_decider(&mut cfg, &parse(user), ConfigLayer::User);
        merge_decider(&mut cfg, &parse(project), ConfigLayer::Project);
        let map: std::collections::HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        apply_decider_env(&mut cfg, |k| map.get(k).cloned());
        resolve_decider(&cfg).0
    }

    /// `build_decider` agrees with `opted_in` across the combinations that
    /// matter: mode, key presence, endpoint layer, and project minimum.
    #[test]
    fn build_decider_is_some_iff_opted_in() {
        let cases: Vec<DeciderSettings> = vec![
            settings("", "", &[]),
            settings("[decider]\nmode = \"auto\"\n", "", &[]),
            settings("[decider]\nmode = \"auto\"\napi_key = \"k\"\n", "", &[]),
            settings(
                "[decider]\nmode = \"auto\"\napi_key = \"k\"\n",
                "[decider]\nmode = \"off\"\n",
                &[],
            ),
            settings(
                "[decider]\nmode = \"shadow\"\napi_key = \"k\"\n",
                "",
                &[("KOTO_DECIDER_ENDPOINT", "http://127.0.0.1:9/x")],
            ),
            settings(
                "",
                "",
                &[
                    ("KOTO_DECIDER", "shadow"),
                    ("KOTO_DECIDER_API_KEY", "k"),
                    ("KOTO_DECIDER_ENDPOINT", "http://127.0.0.1:9/x"),
                ],
            ),
            settings(
                "[decider]\nmode = \"off\"\napi_key = \"k\"\n",
                "",
                &[("KOTO_DECIDER", "auto")],
            ),
        ];
        for s in &cases {
            assert_eq!(build_decider(s).is_some(), s.opted_in(), "{:?}", s);
        }
        assert!(cases.iter().any(|s| s.opted_in()));
        assert!(cases.iter().any(|s| !s.opted_in()));
    }

    #[test]
    fn built_client_is_jev() {
        let s = settings("[decider]\nmode = \"auto\"\napi_key = \"k\"\n", "", &[]);
        assert_eq!(build_decider(&s).unwrap().provider(), "jev");
    }
}
