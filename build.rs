//! Build script — compiles `.proto` schemas into Rust types (via `prost`)
//! with `serde::{Serialize, Deserialize}` derives for YAML/JSON compatibility.
//!
//! Generated code lands in `$OUT_DIR/` and is `include!`-ed from `rust/src/proto/`.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    emit_git_build_info();

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    // Path to our proto source directory.
    let proto_dir = PathBuf::from("proto");

    let protos: &[&str] = &["proto/config.proto", "proto/secrets.proto"];

    let includes: &[&str] = &[
        proto_dir.to_str().unwrap(),
        // prost-build bundles the well-known types (google/protobuf/*.proto)
        // so no extra include is needed for google.protobuf.Timestamp.
    ];

    // -------------------------------------------------------------------
    // prost: compile .proto → Rust structs + enums
    //
    // We derive serde traits directly on every generated type so they can
    // be serialised to / deserialised from YAML and JSON without an
    // additional code-generation layer.
    //
    // IMPORTANT: `#[serde(default)]` can only be applied to structs
    // (protobuf messages), NOT enums. We use `message_attribute` and
    // `enum_attribute` separately to handle this correctly.
    // -------------------------------------------------------------------
    let descriptor_path = out_dir.join("proto_descriptor.bin");

    let mut prost_config = prost_build::Config::new();

    // Derive Serialize + Deserialize on both messages and enums.
    prost_config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");

    // Use snake_case for serde field names (matches YAML / JSON conventions).
    // This is valid on both structs and enums.
    prost_config.type_attribute(".", "#[serde(rename_all = \"snake_case\")]");

    // Default missing fields to their proto defaults instead of failing.
    // This is ONLY valid on structs (messages), not enums.
    prost_config.message_attribute(".", "#[serde(default)]");

    // Emit the file descriptor set for downstream tooling (JSON Schema gen, etc.).
    prost_config.file_descriptor_set_path(&descriptor_path);

    // Map google.protobuf.Timestamp → pbjson_types::Timestamp which already
    // has serde support with proper RFC 3339 string formatting.
    prost_config.extern_path(".google.protobuf.Timestamp", "::pbjson_types::Timestamp");

    prost_config.compile_protos(protos, includes)?;

    // -------------------------------------------------------------------
    // Tell Cargo to re-run when proto files change.
    // -------------------------------------------------------------------
    println!("cargo:rerun-if-changed=proto/");
    for proto in protos {
        println!("cargo:rerun-if-changed={proto}");
    }

    Ok(())
}

fn emit_git_build_info() {
    let git_sha = git_commit_sha().unwrap_or_else(|| "unknown".to_string());
    let git_short_sha = git_short_commit_sha().unwrap_or_else(|| "unknown".to_string());
    let git_date = git_commit_date().unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=HIMITSU_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=HIMITSU_GIT_SHORT_SHA={git_short_sha}");
    println!("cargo:rustc-env=HIMITSU_GIT_DATE={git_date}");

    let Some(git_dir) = git_dir() else {
        return;
    };

    let head_path = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head_path.display());

    let packed_refs = git_dir.join("packed-refs");
    if packed_refs.exists() {
        println!("cargo:rerun-if-changed={}", packed_refs.display());
    }

    if let Ok(head) = std::fs::read_to_string(&head_path) {
        if let Some(reference) = head.strip_prefix("ref: ").map(str::trim) {
            println!(
                "cargo:rerun-if-changed={}",
                git_dir.join(reference).display()
            );
        }
    }
}

fn git_commit_sha() -> Option<String> {
    git_output(&["rev-parse", "HEAD"])
}

fn git_short_commit_sha() -> Option<String> {
    git_output(&["rev-parse", "--short", "HEAD"])
}

fn git_commit_date() -> Option<String> {
    git_output(&["log", "-1", "--date=short", "--format=%cd", "HEAD"])
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn git_dir() -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let git_dir = String::from_utf8(output.stdout).ok()?;
    let git_dir = git_dir.trim();
    if git_dir.is_empty() {
        None
    } else {
        Some(PathBuf::from(git_dir))
    }
}
