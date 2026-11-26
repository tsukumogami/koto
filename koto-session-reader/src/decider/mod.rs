//! The pure, provider-neutral half of decider support: the decision types,
//! request assembly, evaluation and the consultation record the event log
//! embeds. koto's `decider` module adds the client, the ledger and the
//! report.

pub mod check;
pub mod evaluate;
pub mod record;
pub mod request;
pub mod types;

pub use evaluate::{evaluate, EffectiveModes, Evaluation, FieldEvaluation, FieldOutcome};
pub use record::{
    ConsultationOutcome, DeciderConsultation, FieldConsultation, UnavailableInput,
    UnavailableReason,
};
pub use request::{
    build_request, declared_fields, AssembledInputs, BuildRequestError, DeclaredField,
    DeclaredKind, InputTextError,
};
pub use types::{
    Answer, AnswerOption, ApiKey, Decider, DeciderError, DecisionRequest, DecisionResponse,
    ErrorClass, GlobalMode, LabelledInput, Question, QuestionKind, SettingOrigin,
};
