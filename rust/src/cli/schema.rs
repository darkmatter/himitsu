use std::path::Path;

use clap::{Args, Subcommand, ValueEnum};
use tracing::{debug, info};

use super::Context;
use crate::error::Result;
use crate::proto;

/// Generate and manage JSON schemas for himitsu config files.
#[derive(Debug, Args)]
pub struct SchemaArgs {
    #[command(subcommand)]
    pub command: SchemaCommand,
}

#[derive(Debug, Subcommand)]
pub enum SchemaCommand {
    /// Print a single JSON schema to stdout.
    Dump {
        /// Which schema to print.
        #[arg(value_enum)]
        name: SchemaName,

        /// Pretty-print with indentation (default: true).
        #[arg(long, default_value_t = true)]
        pretty: bool,
    },

    /// Print all JSON schemas to stdout (as a JSON object keyed by name).
    DumpAll {
        /// Pretty-print with indentation (default: true).
        #[arg(long, default_value_t = true)]
        pretty: bool,
    },

    /// Write all JSON schemas to the store's `schemas/` directory.
    Refresh,

    /// List all available schema names.
    List,
}

/// The set of schemas that can be generated from the protobuf definitions.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SchemaName {
    /// Unified himitsu config (.himitsu.yaml).
    Config,
    /// Encrypted secret envelope (on-disk wrapper).
    SecretEnvelope,
    /// Share envelope (transport payload).
    ShareEnvelope,
}

impl SchemaName {
    /// All available schema names, in definition order.
    fn all() -> &'static [SchemaName] {
        &[
            SchemaName::Config,
            SchemaName::SecretEnvelope,
            SchemaName::ShareEnvelope,
        ]
    }

    /// The canonical file name for this schema (without directory).
    fn file_name(self) -> &'static str {
        match self {
            SchemaName::Config => "config.schema.json",
            SchemaName::SecretEnvelope => "secret-envelope.schema.json",
            SchemaName::ShareEnvelope => "share-envelope.schema.json",
        }
    }

    /// Human-readable description shown by `schema list`.
    fn description(self) -> &'static str {
        match self {
            SchemaName::Config => "Unified config (.himitsu.yaml)",
            SchemaName::SecretEnvelope => "Encrypted secret envelope (on-disk wrapper)",
            SchemaName::ShareEnvelope => "Share envelope (transport-agnostic sharing payload)",
        }
    }

    /// Generate the JSON Schema value for this schema name.
    fn generate(self) -> serde_json::Value {
        match self {
            SchemaName::Config => proto::config_json_schema(),
            SchemaName::SecretEnvelope => proto::secret_envelope_json_schema(),
            SchemaName::ShareEnvelope => proto::share_envelope_json_schema(),
        }
    }
}

impl std::fmt::Display for SchemaName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaName::Config => write!(f, "config"),
            SchemaName::SecretEnvelope => write!(f, "secret-envelope"),
            SchemaName::ShareEnvelope => write!(f, "share-envelope"),
        }
    }
}

pub fn run(args: SchemaArgs, ctx: &Context) -> Result<()> {
    match args.command {
        SchemaCommand::Dump { name, pretty } => cmd_dump(name, pretty),
        SchemaCommand::DumpAll { pretty } => cmd_dump_all(pretty),
        SchemaCommand::Refresh => cmd_refresh(ctx),
        SchemaCommand::List => cmd_list(),
    }
}

/// Print a single schema to stdout.
fn cmd_dump(name: SchemaName, pretty: bool) -> Result<()> {
    let schema = name.generate();
    let output = format_json(&schema, pretty)?;
    println!("{output}");
    Ok(())
}

/// Print all schemas as a JSON object keyed by canonical name.
fn cmd_dump_all(pretty: bool) -> Result<()> {
    let mut all = serde_json::Map::new();
    for name in SchemaName::all() {
        all.insert(name.to_string(), name.generate());
    }
    let combined = serde_json::Value::Object(all);
    let output = format_json(&combined, pretty)?;
    println!("{output}");
    Ok(())
}

/// Write all schemas as individual JSON files into the store's `schemas/` dir.
fn cmd_refresh(ctx: &Context) -> Result<()> {
    let schemas_dir = ctx.store.join("schemas");
    std::fs::create_dir_all(&schemas_dir)?;
    debug!("writing schemas to {}", schemas_dir.display());

    for name in SchemaName::all() {
        let schema = name.generate();
        let json = serde_json::to_string_pretty(&schema)?;
        let dest = schemas_dir.join(name.file_name());
        std::fs::write(&dest, json.as_bytes())?;
        info!("wrote {}", dest.display());
        println!("  wrote {}", dest.display());
    }

    // Also write a convenience index file that references all schemas.
    write_schema_index(&schemas_dir)?;

    println!(
        "refreshed {} schemas in {}",
        SchemaName::all().len(),
        schemas_dir.display()
    );
    Ok(())
}

/// List available schema names and their descriptions.
fn cmd_list() -> Result<()> {
    println!("Available schemas:\n");
    for name in SchemaName::all() {
        println!("  {:<20} {}", name, name.description());
        println!("  {:<20} file: {}", "", name.file_name());
        println!();
    }
    Ok(())
}

/// Write a `_index.json` file that lists all schema files for tooling consumption.
fn write_schema_index(schemas_dir: &Path) -> Result<()> {
    let entries: Vec<serde_json::Value> = SchemaName::all()
        .iter()
        .map(|name| {
            serde_json::json!({
                "name": name.to_string(),
                "file": name.file_name(),
                "description": name.description(),
            })
        })
        .collect();

    let index = serde_json::json!({
        "$comment": "Auto-generated by `himitsu schema refresh`. Do not edit.",
        "schemas": entries,
    });

    let json = serde_json::to_string_pretty(&index)?;
    let dest = schemas_dir.join("_index.json");
    std::fs::write(&dest, json.as_bytes())?;
    debug!("wrote schema index at {}", dest.display());
    Ok(())
}

/// Serialize a JSON value to a string, optionally pretty-printed.
fn format_json(value: &serde_json::Value, pretty: bool) -> Result<String> {
    if pretty {
        serde_json::to_string_pretty(value).map_err(Into::into)
    } else {
        serde_json::to_string(value).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_schemas_generate_valid_json() {
        for name in SchemaName::all() {
            let schema = name.generate();
            // Must have $schema and title fields
            assert!(
                schema.get("$schema").is_some(),
                "{name} schema missing $schema"
            );
            assert!(schema.get("title").is_some(), "{name} schema missing title");
            assert_eq!(
                schema["type"], "object",
                "{name} schema type must be object"
            );
            // Must serialise without error
            let json = serde_json::to_string_pretty(&schema).unwrap();
            assert!(!json.is_empty());
        }
    }

    #[test]
    fn schema_names_are_unique() {
        let names: Vec<String> = SchemaName::all().iter().map(|n| n.to_string()).collect();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "duplicate schema names found");
    }

    #[test]
    fn schema_file_names_are_unique() {
        let fnames: Vec<&str> = SchemaName::all().iter().map(|n| n.file_name()).collect();
        let mut deduped = fnames.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(
            fnames.len(),
            deduped.len(),
            "duplicate schema file names found"
        );
    }

    #[test]
    fn refresh_writes_files() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(".himitsu");
        std::fs::create_dir_all(&store).unwrap();

        let ctx = Context {
            data_dir: tmp.path().to_path_buf(),
            state_dir: tmp.path().join("state"),
            store: store.clone(),
            recipients_path: None,
        };

        cmd_refresh(&ctx).unwrap();

        let schemas_dir = store.join("schemas");
        assert!(schemas_dir.exists());

        for name in SchemaName::all() {
            let path = schemas_dir.join(name.file_name());
            assert!(path.exists(), "missing schema file: {}", name.file_name());

            let content = std::fs::read_to_string(&path).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
            assert!(parsed.get("$schema").is_some());
        }

        // Index file should also exist
        let index_path = schemas_dir.join("_index.json");
        assert!(index_path.exists());
        let index_content = std::fs::read_to_string(&index_path).unwrap();
        let index: serde_json::Value = serde_json::from_str(&index_content).unwrap();
        let entries = index["schemas"].as_array().unwrap();
        assert_eq!(entries.len(), SchemaName::all().len());
    }

    #[test]
    fn format_json_pretty_vs_compact() {
        let val = serde_json::json!({"a": 1, "b": [2, 3]});

        let pretty = format_json(&val, true).unwrap();
        assert!(pretty.contains('\n'), "pretty output should have newlines");

        let compact = format_json(&val, false).unwrap();
        assert!(
            !compact.contains('\n'),
            "compact output should not have newlines"
        );

        // Both should round-trip to the same value
        let p: serde_json::Value = serde_json::from_str(&pretty).unwrap();
        let c: serde_json::Value = serde_json::from_str(&compact).unwrap();
        assert_eq!(p, c);
    }

    #[test]
    fn dump_all_produces_keyed_object() {
        let mut all = serde_json::Map::new();
        for name in SchemaName::all() {
            all.insert(name.to_string(), name.generate());
        }
        let combined = serde_json::Value::Object(all);

        assert!(combined.is_object());
        let obj = combined.as_object().unwrap();
        assert_eq!(obj.len(), SchemaName::all().len());

        // Each key should map to a valid schema
        for (key, value) in obj {
            assert!(
                value.get("$schema").is_some(),
                "schema for {key} missing $schema"
            );
        }
    }
}
