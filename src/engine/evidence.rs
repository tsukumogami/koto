use std::collections::BTreeMap;
use std::fmt;

use crate::template::types::FieldSchema;

/// Per-field validation error detail.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldError {
    pub field: String,
    pub reason: String,
}

/// Domain error for evidence validation failures.
///
/// Contains all field-level errors collected without short-circuiting.
/// The CLI layer maps this to `NextError` with `InvalidSubmission` code.
#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceValidationError {
    pub field_errors: Vec<FieldError>,
}

impl fmt::Display for EvidenceValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "evidence validation failed: ")?;
        let reasons: Vec<&str> = self.field_errors.iter().map(|e| e.reason.as_str()).collect();
        write!(f, "{}", reasons.join("; "))
    }
}

impl std::error::Error for EvidenceValidationError {}

/// Validate a JSON evidence payload against an accepts schema.
///
/// Checks are collected without short-circuiting so the caller sees every
/// problem in one response. Validations performed:
///
/// - Required fields must be present
/// - Field types must match: `string` -> JSON string, `number` -> JSON number,
///   `boolean` -> JSON bool, `enum` -> JSON string matching one of `FieldSchema.values`
/// - Unknown fields (not declared in accepts) are rejected
pub fn validate_evidence(
    data: &serde_json::Value,
    accepts: &BTreeMap<String, FieldSchema>,
) -> Result<(), EvidenceValidationError> {
    let mut errors: Vec<FieldError> = Vec::new();

    let obj = match data.as_object() {
        Some(o) => o,
        None => {
            errors.push(FieldError {
                field: "(root)".to_string(),
                reason: "evidence must be a JSON object".to_string(),
            });
            return Err(EvidenceValidationError {
                field_errors: errors,
            });
        }
    };

    // Check for unknown fields.
    for key in obj.keys() {
        if !accepts.contains_key(key) {
            errors.push(FieldError {
                field: key.clone(),
                reason: format!("unknown field {:?}", key),
            });
        }
    }

    // Check each declared field.
    for (field_name, schema) in accepts {
        match obj.get(field_name) {
            None => {
                if schema.required {
                    errors.push(FieldError {
                        field: field_name.clone(),
                        reason: "required field missing".to_string(),
                    });
                }
            }
            Some(value) => {
                validate_field_type(field_name, value, schema, &mut errors);
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(EvidenceValidationError {
            field_errors: errors,
        })
    }
}

/// Validate that a JSON value matches the expected field type.
fn validate_field_type(
    field_name: &str,
    value: &serde_json::Value,
    schema: &FieldSchema,
    errors: &mut Vec<FieldError>,
) {
    match schema.field_type.as_str() {
        "string" => {
            if !value.is_string() {
                errors.push(FieldError {
                    field: field_name.to_string(),
                    reason: format!("expected string, got {}", json_type_name(value)),
                });
            }
        }
        "number" => {
            if !value.is_number() {
                errors.push(FieldError {
                    field: field_name.to_string(),
                    reason: format!("expected number, got {}", json_type_name(value)),
                });
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                errors.push(FieldError {
                    field: field_name.to_string(),
                    reason: format!("expected boolean, got {}", json_type_name(value)),
                });
            }
        }
        "enum" => {
            match value.as_str() {
                Some(s) => {
                    if !schema.values.contains(&s.to_string()) {
                        errors.push(FieldError {
                            field: field_name.to_string(),
                            reason: format!(
                                "value {:?} is not in allowed values {:?}",
                                s, schema.values
                            ),
                        });
                    }
                }
                None => {
                    errors.push(FieldError {
                        field: field_name.to_string(),
                        reason: format!("expected string for enum, got {}", json_type_name(value)),
                    });
                }
            }
        }
        _ => {
            // Unsupported field type -- this should be caught by template validation,
            // but handle it defensively.
            errors.push(FieldError {
                field: field_name.to_string(),
                reason: format!("unsupported field type {:?}", schema.field_type),
            });
        }
    }
}

/// Return a human-readable name for a JSON value type.
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_schema(
        field_type: &str,
        required: bool,
        values: Vec<&str>,
    ) -> FieldSchema {
        FieldSchema {
            field_type: field_type.to_string(),
            required,
            values: values.into_iter().map(|s| s.to_string()).collect(),
            description: String::new(),
        }
    }

    #[test]
    fn valid_payload_accepted() {
        let mut accepts = BTreeMap::new();
        accepts.insert("name".to_string(), make_schema("string", true, vec![]));
        accepts.insert("count".to_string(), make_schema("number", false, vec![]));
        accepts.insert("active".to_string(), make_schema("boolean", false, vec![]));

        let data = serde_json::json!({
            "name": "test",
            "count": 42,
            "active": true
        });

        assert!(validate_evidence(&data, &accepts).is_ok());
    }

    #[test]
    fn valid_payload_optional_fields_omitted() {
        let mut accepts = BTreeMap::new();
        accepts.insert("name".to_string(), make_schema("string", true, vec![]));
        accepts.insert("notes".to_string(), make_schema("string", false, vec![]));

        let data = serde_json::json!({"name": "test"});

        assert!(validate_evidence(&data, &accepts).is_ok());
    }

    #[test]
    fn missing_required_field() {
        let mut accepts = BTreeMap::new();
        accepts.insert("decision".to_string(), make_schema("string", true, vec![]));

        let data = serde_json::json!({});

        let err = validate_evidence(&data, &accepts).unwrap_err();
        assert_eq!(err.field_errors.len(), 1);
        assert_eq!(err.field_errors[0].field, "decision");
        assert!(err.field_errors[0].reason.contains("required field missing"));
    }

    #[test]
    fn wrong_type_string() {
        let mut accepts = BTreeMap::new();
        accepts.insert("name".to_string(), make_schema("string", true, vec![]));

        let data = serde_json::json!({"name": 42});

        let err = validate_evidence(&data, &accepts).unwrap_err();
        assert_eq!(err.field_errors.len(), 1);
        assert_eq!(err.field_errors[0].field, "name");
        assert!(err.field_errors[0].reason.contains("expected string"));
        assert!(err.field_errors[0].reason.contains("number"));
    }

    #[test]
    fn wrong_type_number() {
        let mut accepts = BTreeMap::new();
        accepts.insert("count".to_string(), make_schema("number", true, vec![]));

        let data = serde_json::json!({"count": "not a number"});

        let err = validate_evidence(&data, &accepts).unwrap_err();
        assert_eq!(err.field_errors.len(), 1);
        assert_eq!(err.field_errors[0].field, "count");
        assert!(err.field_errors[0].reason.contains("expected number"));
        assert!(err.field_errors[0].reason.contains("string"));
    }

    #[test]
    fn wrong_type_boolean() {
        let mut accepts = BTreeMap::new();
        accepts.insert("active".to_string(), make_schema("boolean", true, vec![]));

        let data = serde_json::json!({"active": "yes"});

        let err = validate_evidence(&data, &accepts).unwrap_err();
        assert_eq!(err.field_errors.len(), 1);
        assert_eq!(err.field_errors[0].field, "active");
        assert!(err.field_errors[0].reason.contains("expected boolean"));
        assert!(err.field_errors[0].reason.contains("string"));
    }

    #[test]
    fn enum_valid_value() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            make_schema("enum", true, vec!["proceed", "escalate"]),
        );

        let data = serde_json::json!({"decision": "proceed"});

        assert!(validate_evidence(&data, &accepts).is_ok());
    }

    #[test]
    fn enum_value_mismatch() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            make_schema("enum", true, vec!["proceed", "escalate"]),
        );

        let data = serde_json::json!({"decision": "invalid"});

        let err = validate_evidence(&data, &accepts).unwrap_err();
        assert_eq!(err.field_errors.len(), 1);
        assert_eq!(err.field_errors[0].field, "decision");
        assert!(err.field_errors[0].reason.contains("not in allowed values"));
    }

    #[test]
    fn enum_wrong_type() {
        let mut accepts = BTreeMap::new();
        accepts.insert(
            "decision".to_string(),
            make_schema("enum", true, vec!["proceed", "escalate"]),
        );

        let data = serde_json::json!({"decision": 42});

        let err = validate_evidence(&data, &accepts).unwrap_err();
        assert_eq!(err.field_errors.len(), 1);
        assert!(err.field_errors[0].reason.contains("expected string for enum"));
    }

    #[test]
    fn unknown_field_rejected() {
        let mut accepts = BTreeMap::new();
        accepts.insert("name".to_string(), make_schema("string", true, vec![]));

        let data = serde_json::json!({"name": "test", "extra": "field"});

        let err = validate_evidence(&data, &accepts).unwrap_err();
        assert_eq!(err.field_errors.len(), 1);
        assert_eq!(err.field_errors[0].field, "extra");
        assert!(err.field_errors[0].reason.contains("unknown field"));
    }

    #[test]
    fn multiple_errors_collected() {
        let mut accepts = BTreeMap::new();
        accepts.insert("name".to_string(), make_schema("string", true, vec![]));
        accepts.insert("count".to_string(), make_schema("number", true, vec![]));
        accepts.insert(
            "decision".to_string(),
            make_schema("enum", true, vec!["yes", "no"]),
        );

        let data = serde_json::json!({
            "count": "not a number",
            "decision": "maybe",
            "unknown": true
        });

        let err = validate_evidence(&data, &accepts).unwrap_err();
        // Expect: unknown field "unknown", missing required "name",
        //         wrong type for "count", invalid enum value for "decision"
        assert_eq!(err.field_errors.len(), 4);

        let fields: Vec<&str> = err.field_errors.iter().map(|e| e.field.as_str()).collect();
        assert!(fields.contains(&"unknown"));
        assert!(fields.contains(&"name"));
        assert!(fields.contains(&"count"));
        assert!(fields.contains(&"decision"));
    }

    #[test]
    fn non_object_payload_rejected() {
        let accepts = BTreeMap::new();
        let data = serde_json::json!("not an object");

        let err = validate_evidence(&data, &accepts).unwrap_err();
        assert_eq!(err.field_errors.len(), 1);
        assert!(err.field_errors[0].reason.contains("must be a JSON object"));
    }

    #[test]
    fn empty_payload_with_no_required_fields() {
        let mut accepts = BTreeMap::new();
        accepts.insert("notes".to_string(), make_schema("string", false, vec![]));

        let data = serde_json::json!({});

        assert!(validate_evidence(&data, &accepts).is_ok());
    }

    #[test]
    fn display_implementation() {
        let err = EvidenceValidationError {
            field_errors: vec![
                FieldError {
                    field: "name".to_string(),
                    reason: "required field missing".to_string(),
                },
                FieldError {
                    field: "count".to_string(),
                    reason: "expected number, got string".to_string(),
                },
            ],
        };

        let msg = err.to_string();
        assert!(msg.contains("evidence validation failed"));
        assert!(msg.contains("required field missing"));
        assert!(msg.contains("expected number, got string"));
    }
}
