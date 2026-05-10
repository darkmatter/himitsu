//! Protobuf-generated types for himitsu schemas.
//!
//! The `.proto` source files live in `<repo>/proto/` and are compiled by
//! `build.rs` (prost + pbjson) into Rust structs, enums, and serde impls.
//!
//! This module re-exports the generated code and provides ergonomic helpers
//! used by the CLI (string/enum conversions + JSON Schema emitters).

// -----------------------------------------------------------------------
// Include generated code
// -----------------------------------------------------------------------

/// Config schema types generated from `proto/config.proto`.
pub mod config {
    include!(concat!(env!("OUT_DIR"), "/himitsu.config.rs"));
}

/// Secrets schema types generated from `proto/secrets.proto`.
pub mod secrets {
    include!(concat!(env!("OUT_DIR"), "/himitsu.secrets.rs"));
}

/// CLI command argument schemas generated from `proto/commands.proto`.
///
/// These messages back the generic TUI form widget — see
/// [`crate::tui::forms`] for the `ProtoForm` trait that maps each message
/// onto a labelled, validated form view.
pub mod commands {
    include!(concat!(env!("OUT_DIR"), "/himitsu.commands.rs"));
}

// -----------------------------------------------------------------------
// Re-exports for convenience
// -----------------------------------------------------------------------

pub use config::{CodegenLang, Config as ProtoConfig, Identity, Policy as ProtoPolicy, Remote};

pub use secrets::{RecipientInfo, SecretEntry, SecretEnvelope, SecretValue, StoreManifest};

// -----------------------------------------------------------------------
// Enum ↔ string helpers
// -----------------------------------------------------------------------

/// Parse a `CodegenLang` from string.
pub fn codegen_lang_from_str(s: &str) -> CodegenLang {
    match s.to_lowercase().as_str() {
        "typescript" | "ts" => CodegenLang::Typescript,
        "golang" | "go" => CodegenLang::Golang,
        "python" | "py" => CodegenLang::Python,
        "rust" | "rs" => CodegenLang::Rust,
        _ => CodegenLang::Unspecified,
    }
}

/// Convert a `CodegenLang` into canonical string form.
pub fn codegen_lang_to_str(l: CodegenLang) -> &'static str {
    match l {
        CodegenLang::Typescript => "typescript",
        CodegenLang::Golang => "golang",
        CodegenLang::Python => "python",
        CodegenLang::Rust => "rust",
        CodegenLang::Unspecified => "unspecified",
    }
}

// -----------------------------------------------------------------------
// JSON Schema generators
// -----------------------------------------------------------------------

/// Unified config schema (`.himitsu.yaml`).
pub fn config_json_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://himitsu.dev/schemas/config.schema.json",
        "title": "Himitsu Config",
        "description": "Unified himitsu configuration (identity, policies, imports, codegen).",
        "type": "object",
        "required": ["identity"],
        "properties": {
            "identity": {
                "type": "object",
                "description": "Identity for this secrets store (person or organization).",
                "required": ["public_keys"],
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Human-readable identity name."
                    },
                    "public_keys": {
                        "type": "array",
                        "description": "Age public keys that should always be recipients.",
                        "items": { "type": "string" },
                        "minItems": 1
                    }
                },
                "additionalProperties": false
            },
            "policies": {
                "type": "array",
                "description": "Recipient policy rules by path pattern.",
                "items": {
                    "type": "object",
                    "required": ["path_pattern"],
                    "properties": {
                        "path_pattern": {
                            "type": "string",
                            "description": "Path pattern this rule applies to."
                        },
                        "include": {
                            "type": "array",
                            "description": "Included recipient selectors.",
                            "items": { "type": "string" }
                        },
                        "exclude": {
                            "type": "array",
                            "description": "Excluded recipient selectors.",
                            "items": { "type": "string" }
                        }
                    },
                    "additionalProperties": false
                }
            },
            "imports": {
                "type": "array",
                "description": "Imported secret stores (typically git submodules).",
                "items": {
                    "type": "object",
                    "required": ["type", "ref", "path"],
                    "properties": {
                        "type": {
                            "type": "string",
                            "description": "Remote type (e.g. github, gitlab)."
                        },
                        "ref": {
                            "type": "string",
                            "description": "Remote URL or identifier."
                        },
                        "path": {
                            "type": "string",
                            "description": "Local path prefix to import into."
                        }
                    },
                    "additionalProperties": false
                }
            },
            "enable_audits": {
                "type": "boolean",
                "description": "Enable audit log appends on mutations."
            },
            "codegen": {
                "type": "object",
                "description": "Typed accessor code generation settings.",
                "required": ["lang", "path"],
                "properties": {
                    "lang": {
                        "type": "string",
                        "enum": ["typescript", "golang", "python", "rust"],
                        "description": "Codegen target language."
                    },
                    "path": {
                        "type": "string",
                        "description": "Output path for generated code."
                    }
                },
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

/// Encrypted secret envelope schema.
pub fn secret_envelope_json_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://himitsu.dev/schemas/secret-envelope.schema.json",
        "title": "Himitsu Secret Envelope",
        "description": "Encrypted secret wrapper containing age ciphertext and metadata.",
        "type": "object",
        "required": ["version", "key_name", "environment", "ciphertext"],
        "properties": {
            "version": { "type": "integer", "minimum": 1 },
            "key_name": { "type": "string" },
            "environment": { "type": "string" },
            "ciphertext": { "type": "string", "description": "Base64-encoded age ciphertext." },
            "recipients": {
                "type": "array",
                "items": { "type": "string" }
            },
            "encrypted_at": { "type": "string", "format": "date-time" },
            "encrypted_by": { "type": "string" },
            "content_hash": { "type": "string" }
        },
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codegen_lang_str_roundtrip() {
        assert_eq!(codegen_lang_from_str("typescript"), CodegenLang::Typescript);
        assert_eq!(codegen_lang_from_str("ts"), CodegenLang::Typescript);
        assert_eq!(codegen_lang_from_str("golang"), CodegenLang::Golang);
        assert_eq!(codegen_lang_from_str("go"), CodegenLang::Golang);
        assert_eq!(codegen_lang_from_str("python"), CodegenLang::Python);
        assert_eq!(codegen_lang_from_str("py"), CodegenLang::Python);
        assert_eq!(codegen_lang_from_str("rust"), CodegenLang::Rust);
        assert_eq!(codegen_lang_from_str("rs"), CodegenLang::Rust);
        assert_eq!(codegen_lang_from_str("???"), CodegenLang::Unspecified);

        assert_eq!(codegen_lang_to_str(CodegenLang::Typescript), "typescript");
        assert_eq!(codegen_lang_to_str(CodegenLang::Golang), "golang");
        assert_eq!(codegen_lang_to_str(CodegenLang::Python), "python");
        assert_eq!(codegen_lang_to_str(CodegenLang::Rust), "rust");
        assert_eq!(codegen_lang_to_str(CodegenLang::Unspecified), "unspecified");
    }

    #[test]
    fn config_schema_is_valid_json() {
        let schema = config_json_schema();
        assert_eq!(schema["title"], "Himitsu Config");
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["identity"].is_object());
        assert!(schema["properties"]["imports"].is_object());
        assert!(schema["properties"]["policies"].is_object());
    }

    #[test]
    fn secret_envelope_schema_has_ciphertext() {
        let schema = secret_envelope_json_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("ciphertext")));
    }
}
