use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use prost::Message;
use serde_yaml::{Mapping, Value};
use tempfile::NamedTempFile;

use crate::crypto::{age, secret_value, tags::validate_tag};
use crate::error::{HimitsuError, Result};
use crate::proto;
use crate::remote::store;

#[derive(Debug, Args)]
pub struct MigrateArgs {
    #[command(subcommand)]
    pub cmd: MigrateCommand,
}

#[derive(Debug, Subcommand)]
pub enum MigrateCommand {
    /// One-shot migration: fold environment proto fields into tags; rewrite .himitsu.yaml envs: → outputs:.
    Envs {
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run(args: MigrateArgs, ctx: &super::Context) -> Result<()> {
    match args.cmd {
        MigrateCommand::Envs { dry_run } => migrate_envs(ctx, dry_run),
    }
}

fn migrate_envs(ctx: &super::Context, dry_run: bool) -> Result<()> {
    let secrets_migrated = migrate_secret_environments(ctx, dry_run)?;
    let output_blocks_rewritten = migrate_project_config(&ctx.store, dry_run)?;
    let cache_path = ctx.state_dir.join("envs.db");
    let cache_deleted = cache_path.exists();
    if cache_deleted && !dry_run {
        std::fs::remove_file(&cache_path)?;
    }

    println!("himitsu migrate envs summary");
    println!("  secrets migrated: {secrets_migrated}");
    println!("  output blocks rewritten: {output_blocks_rewritten}");
    println!("  cache deleted: {cache_deleted}");
    println!("  dry-run: {dry_run}");

    Ok(())
}

#[allow(deprecated)]
fn migrate_secret_environments(ctx: &super::Context, dry_run: bool) -> Result<usize> {
    let age_files = collect_age_files(&ctx.store)?;
    if age_files.is_empty() {
        return Ok(0);
    }

    let identities = ctx.load_identities()?;
    let recipients = age::collect_recipients(&ctx.store, ctx.recipients_path.as_deref())?;
    if recipients.is_empty() {
        return Err(HimitsuError::Recipient("no recipients found".into()));
    }

    let mut migrated = 0;
    for path in age_files {
        let bytes = std::fs::read(&path)?;
        let Ok(mut envelope) = proto::SecretEnvelope::decode(bytes.as_slice()) else {
            continue;
        };
        if envelope.environment.is_empty() || envelope.ciphertext.is_empty() {
            continue;
        }
        if validate_tag(&envelope.environment).is_err() {
            eprintln!(
                "warning: skipping {}: legacy environment {:?} is not a valid tag",
                path.display(),
                envelope.environment
            );
            continue;
        }

        let plaintext = match age::decrypt_with_identities(&envelope.ciphertext, &identities) {
            Ok(plaintext) => plaintext,
            Err(_) => envelope.ciphertext.clone(),
        };
        let decoded = secret_value::decode_with_legacy_environment(
            &plaintext,
            Some(envelope.environment.as_str()),
        );
        let value = proto::SecretValue {
            data: decoded.data,
            content_type: String::new(),
            annotations: decoded.annotations,
            totp: decoded.totp,
            url: decoded.url,
            expires_at: decoded.expires_at,
            description: decoded.description,
            env_key: decoded.env_key,
            tags: decoded.tags,
        };
        envelope.ciphertext = age::encrypt(&secret_value::encode(&value), &recipients)?;
        envelope.environment.clear();

        if !dry_run {
            atomic_write(&path, &envelope.encode_to_vec())?;
        }
        migrated += 1;
    }

    Ok(migrated)
}

fn collect_age_files(store: &Path) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(&path, out)?;
            } else if path.extension().is_some_and(|ext| ext == "age") {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut paths = Vec::new();
    walk(&store::secrets_dir(store), &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn migrate_project_config(store: &Path, dry_run: bool) -> Result<usize> {
    let path = store.join(".himitsu.yaml");
    if !path.exists() {
        return Ok(0);
    }

    let contents = std::fs::read_to_string(&path)?;
    let mut root: Value = if contents.trim().is_empty() {
        Value::Mapping(Mapping::new())
    } else {
        serde_yaml::from_str(&contents)?
    };
    let Some(root_map) = root.as_mapping_mut() else {
        return Err(HimitsuError::InvalidConfig(format!(
            "{} must be a YAML mapping",
            path.display()
        )));
    };

    let Some(envs) = root_map.remove(Value::String("envs".to_string())) else {
        return Ok(0);
    };
    let outputs = translate_envs_to_outputs(&envs)?;
    let rewritten = outputs.as_mapping().map_or(0, Mapping::len);
    root_map.insert(Value::String("outputs".to_string()), outputs);

    if !dry_run {
        let backup_path = path.with_file_name(".himitsu.yaml.bak");
        atomic_write(&backup_path, contents.as_bytes())?;
        atomic_write(&path, serde_yaml::to_string(&root)?.as_bytes())?;
    }

    Ok(rewritten)
}

fn translate_envs_to_outputs(envs: &Value) -> Result<Value> {
    let envs_map = envs.as_mapping().ok_or_else(|| {
        HimitsuError::InvalidConfig("legacy `envs` block must be a mapping".into())
    })?;
    let mut outputs = Mapping::new();

    for (name, entries) in envs_map {
        let sequence = entries.as_sequence().ok_or_else(|| {
            HimitsuError::InvalidConfig("legacy env entries must be YAML sequences".into())
        })?;
        let mut selectors = Vec::new();
        let mut tag_selectors = Vec::new();
        let mut aliases = Mapping::new();

        for entry in sequence {
            if let Some(selector) = entry.as_str() {
                if selector.starts_with("tag:") {
                    tag_selectors.push(selector.to_string());
                } else {
                    selectors.push(Value::String(selector.to_string()));
                }
            } else if let Some(map) = entry.as_mapping() {
                if map.len() != 1 {
                    return Err(HimitsuError::InvalidConfig(
                        "legacy env alias entries must have exactly one key".into(),
                    ));
                }
                for (alias, selector) in map {
                    aliases.insert(alias.clone(), selector.clone());
                }
            } else {
                return Err(HimitsuError::InvalidConfig(
                    "legacy env entry must be a string or single-key map".into(),
                ));
            }
        }

        if !tag_selectors.is_empty() {
            selectors.push(Value::String(tag_selectors.join("+")));
        }

        let mut output = Mapping::new();
        output.insert(
            Value::String("selectors".to_string()),
            Value::Sequence(selectors),
        );
        output.insert(
            Value::String("aliases".to_string()),
            Value::Mapping(aliases),
        );
        outputs.insert(name.clone(), Value::Mapping(output));
    }

    Ok(Value::Mapping(outputs))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| {
        HimitsuError::InvalidReference(format!("{} has no parent directory", path.display()))
    })?;
    let mut tmp = NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, bytes)?;
    tmp.persist(path).map_err(|err| err.error)?;
    Ok(())
}
