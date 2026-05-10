use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};

use super::Context;
use crate::crypto::age;
use crate::error::{HimitsuError, Result};
use crate::remote::store as rstore;

/// Manage recipients.
#[derive(Debug, Args)]
pub struct RecipientArgs {
    #[command(subcommand)]
    pub command: RecipientCommand,
}

#[derive(Debug, Subcommand)]
pub enum RecipientCommand {
    /// Add a recipient.
    ///
    /// Recipients live under `<store>/.himitsu/recipients/<name>.pub`.
    /// Names may contain `/` to create a path-based hierarchy
    /// (e.g. `ops/alice` → `.himitsu/recipients/ops/alice.pub`).
    ///
    /// Examples:
    ///   himitsu recipient add laptop --self
    ///   himitsu recipient add ops/alice --age-key age1... --description "Alice"
    Add {
        /// Recipient name (e.g. laptop, ops/alice).
        name: String,
        /// Add yourself as a recipient (reads the local age public key).
        #[arg(long = "self")]
        self_: bool,
        /// Explicit age public key (e.g. age1xxxxxxx...).
        #[arg(long)]
        age_key: Option<String>,
        /// Optional human-readable description (stored as sidecar metadata).
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove a recipient (deletes pub + sidecar).
    Rm {
        /// Name of the recipient to remove (e.g. `ops/alice`).
        name: String,
    },
    /// Show a recipient's key and description.
    Show {
        /// Recipient name to look up (e.g. `ops/alice`).
        name: String,
    },
    /// List recipients in a plain aligned table.
    Ls,
}

/// Sidecar metadata stored beside each `<name>.pub`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecipientMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
}

#[derive(Debug, Clone)]
struct RecipientRow {
    name: String,
    description: String,
    key: String,
    short_key: String,
}

pub fn run(args: RecipientArgs, ctx: &Context) -> Result<()> {
    match args.command {
        RecipientCommand::Add {
            name,
            self_,
            age_key,
            description,
        } => add(ctx, &name, self_, age_key.as_deref(), description),

        RecipientCommand::Rm { name } => rm(ctx, &name),

        RecipientCommand::Show { name } => show(ctx, &name),

        RecipientCommand::Ls => ls(ctx),
    }
}

// ── add ─────────────────────────────────────────────────────────────────────

fn add(
    ctx: &Context,
    name: &str,
    self_: bool,
    age_key: Option<&str>,
    description: Option<String>,
) -> Result<()> {
    validate_name(name)?;

    let pubkey = if self_ {
        let key_path = ctx.key_path();
        let contents = std::fs::read_to_string(&key_path)?;
        extract_public_key(&contents).ok_or_else(|| {
            HimitsuError::Recipient("cannot extract public key from key file".into())
        })?
    } else if let Some(key) = age_key {
        age::parse_recipient(key)?;
        key.to_string()
    } else {
        return Err(HimitsuError::Recipient(
            "either --self or --age-key must be provided".into(),
        ));
    };

    let recipients_dir = flat_recipients_dir(ctx);
    std::fs::create_dir_all(&recipients_dir)?;

    let pub_file = recipients_dir.join(format!("{name}.pub"));
    // Create intermediate directories for path-based names (e.g. ops/alice).
    if let Some(parent) = pub_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if pub_file.exists() {
        return Err(HimitsuError::Recipient(format!(
            "recipient '{name}' already exists at {}",
            pub_file.display()
        )));
    }
    std::fs::write(&pub_file, format!("{pubkey}\n"))?;

    // Resolve description: explicit flag wins, otherwise prompt if interactive.
    let final_description = match description {
        Some(d) => {
            let trimmed = d.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        None if std::io::stdin().is_terminal() => prompt_description()?,
        None => None,
    };

    if final_description.is_some() {
        let sidecar_path = recipients_dir.join(format!("{name}.yaml"));
        let meta = RecipientMeta {
            description: final_description.clone(),
            added_at: Some(now_iso8601()),
        };
        std::fs::write(&sidecar_path, serde_yaml::to_string(&meta)?)?;
    }

    println!("Added recipient '{name}'");
    Ok(())
}

fn prompt_description() -> Result<Option<String>> {
    eprint!("Description (optional, press enter to skip): ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed))
    }
}

// ── rm ──────────────────────────────────────────────────────────────────────

fn rm(ctx: &Context, name: &str) -> Result<()> {
    let recipients_dir = flat_recipients_dir(ctx);

    let pub_file = recipients_dir.join(format!("{name}.pub"));
    if !pub_file.exists() {
        return Err(HimitsuError::Recipient(format!(
            "recipient '{name}' not found"
        )));
    }
    std::fs::remove_file(&pub_file)?;
    let sidecar = recipients_dir.join(format!("{name}.yaml"));
    if sidecar.exists() {
        std::fs::remove_file(&sidecar)?;
    }

    println!("Removed recipient '{name}'");
    Ok(())
}

// ── show ────────────────────────────────────────────────────────────────────

fn show(ctx: &Context, name: &str) -> Result<()> {
    let recipients_dir = flat_recipients_dir(ctx);
    let pub_file = recipients_dir.join(format!("{name}.pub"));
    if !pub_file.exists() {
        return Err(HimitsuError::Recipient(format!(
            "recipient '{name}' not found"
        )));
    }
    let key = std::fs::read_to_string(&pub_file)?;
    let key = key.trim();
    let meta = read_sidecar(&recipients_dir, name);

    println!("Name:         {name}");
    println!("Public key:   {key}");
    if let Some(desc) = meta.description.as_deref().filter(|d| !d.is_empty()) {
        println!("Description:  {desc}");
    }
    if let Some(added) = meta.added_at.as_deref().filter(|d| !d.is_empty()) {
        println!("Added at:     {added}");
    }
    Ok(())
}

// ── ls ──────────────────────────────────────────────────────────────────────

fn ls(ctx: &Context) -> Result<()> {
    let recipients_dir = flat_recipients_dir(ctx);
    if !recipients_dir.exists() {
        return Ok(());
    }

    let mut rows = vec![];
    collect_pub_entries(&recipients_dir, &recipients_dir, &mut rows)?;
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    if rows.is_empty() {
        return Ok(());
    }

    let self_key = self_public_key(ctx);
    print!(
        "{}",
        render_recipient_table(&rows, self_key.as_deref(), std::io::stdout().is_terminal())
    );
    Ok(())
}

/// Recursively collect `.pub` entries, building path-based names relative to `base`.
fn collect_pub_entries(base: &Path, dir: &Path, rows: &mut Vec<RecipientRow>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_pub_entries(base, &entry.path(), rows)?;
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy().to_string();
        let Some(basename) = fname.strip_suffix(".pub") else {
            continue;
        };
        // Build the full path-based name relative to the recipients root.
        let rel = entry
            .path()
            .strip_prefix(base)
            .unwrap_or(entry.path().as_path())
            .with_extension("")
            .to_string_lossy()
            .to_string();
        let name = if rel.is_empty() {
            basename.to_string()
        } else {
            rel
        };
        let key = std::fs::read_to_string(entry.path())?.trim().to_string();
        let meta = read_sidecar(entry.path().parent().unwrap_or(base), basename);
        let description = meta.description.unwrap_or_default();
        let short_key = short_fingerprint(&key);
        rows.push(RecipientRow {
            name,
            description,
            key,
            short_key,
        });
    }
    Ok(())
}

fn render_recipient_table(
    rows: &[RecipientRow],
    self_key: Option<&str>,
    use_color: bool,
) -> String {
    let headers = ("NAME", "DESCRIPTION", "KEY");
    let w0 = rows
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(0)
        .max(headers.0.len());
    let w1 = rows
        .iter()
        .map(|r| r.description.len())
        .max()
        .unwrap_or(0)
        .max(headers.1.len());

    let mut out = String::new();
    out.push_str(&format!(
        "{:<w0$}  {:<w1$}  {}\n",
        headers.0,
        headers.1,
        headers.2,
        w0 = w0,
        w1 = w1,
    ));
    for row in rows {
        let name = format!("{:<w0$}", row.name, w0 = w0);
        let description = format!("{:<w1$}", row.description, w1 = w1);
        let is_self = self_key.is_some_and(|key| key == row.key);
        if use_color && is_self {
            out.push_str(&format!(
                "{}  {}  {}\n",
                name.green().bold(),
                description.green().bold(),
                row.short_key.green().bold(),
            ));
        } else {
            out.push_str(&format!("{name}  {description}  {}\n", row.short_key));
        }
    }
    out
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn flat_recipients_dir(ctx: &Context) -> PathBuf {
    rstore::recipients_dir_with_override(&ctx.store, ctx.recipients_path.as_deref())
}

fn read_sidecar(dir: &Path, name: &str) -> RecipientMeta {
    let path = dir.join(format!("{name}.yaml"));
    if !path.exists() {
        return RecipientMeta::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_yaml::from_str(&c).ok())
        .unwrap_or_default()
}

fn self_public_key(ctx: &Context) -> Option<String> {
    std::fs::read_to_string(ctx.key_path())
        .ok()
        .and_then(|contents| extract_public_key(&contents))
}

fn short_fingerprint(key: &str) -> String {
    // Show the trailing 10 chars of the age public key as a cheap fingerprint.
    let trimmed = key.trim();
    if trimmed.len() <= 14 {
        return trimmed.to_string();
    }
    let tail = &trimmed[trimmed.len().saturating_sub(10)..];
    format!("…{tail}")
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains('\\')
        || name.starts_with('.')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains("..")
        || name.contains("//")
    {
        return Err(HimitsuError::Recipient(format!(
            "invalid recipient name '{name}'"
        )));
    }
    // Validate each path segment individually.
    for segment in name.split('/') {
        if segment.is_empty() || segment.starts_with('.') {
            return Err(HimitsuError::Recipient(format!(
                "invalid recipient name '{name}'"
            )));
        }
    }
    Ok(())
}

fn extract_public_key(contents: &str) -> Option<String> {
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("# public key: ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86400) as i32;
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i32 + era * 400 + if m <= 2 { 1 } else { 0 };
    let hms = secs % 86400;
    let h = hms / 3600;
    let mi = (hms % 3600) / 60;
    let s = hms % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn mk_ctx() -> (TempDir, Context) {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().to_path_buf();
        std::fs::create_dir_all(rstore::secrets_dir(&store)).unwrap();
        std::fs::create_dir_all(rstore::recipients_dir(&store)).unwrap();
        let ctx = Context {
            data_dir: tmp.path().join("data"),
            state_dir: tmp.path().join("state"),
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        };
        (tmp, ctx)
    }

    const AGE_KEY_1: &str = "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";
    const AGE_KEY_2: &str = "age1lvyvwawkr0mcnnnncaghunadrqkmuf9e6507x9y920xxpp866cnql7dp2z";

    #[test]
    fn add_writes_flat_pub_and_errors_on_duplicate() {
        let (_tmp, ctx) = mk_ctx();
        add(&ctx, "alice", false, Some(AGE_KEY_1), Some("hi".into())).unwrap();
        let pub_file = rstore::recipients_dir(&ctx.store).join("alice.pub");
        assert!(pub_file.exists());

        let dup = add(&ctx, "alice", false, Some(AGE_KEY_2), None);
        assert!(dup.is_err());
    }

    #[test]
    fn add_with_description_writes_sidecar() {
        let (_tmp, ctx) = mk_ctx();
        add(
            &ctx,
            "alice",
            false,
            Some(AGE_KEY_1),
            Some("Alice from platform".into()),
        )
        .unwrap();
        let sidecar = rstore::recipients_dir(&ctx.store).join("alice.yaml");
        assert!(sidecar.exists());
        let meta: RecipientMeta =
            serde_yaml::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(meta.description.as_deref(), Some("Alice from platform"));
        assert!(meta.added_at.is_some());
    }

    #[test]
    fn rm_deletes_pub_and_sidecar() {
        let (_tmp, ctx) = mk_ctx();
        add(&ctx, "alice", false, Some(AGE_KEY_1), Some("desc".into())).unwrap();

        rm(&ctx, "alice").unwrap();

        let rdir = rstore::recipients_dir(&ctx.store);
        assert!(!rdir.join("alice.pub").exists());
        assert!(!rdir.join("alice.yaml").exists());
    }

    #[test]
    fn add_path_based_recipient_creates_subdirs() {
        let (_tmp, ctx) = mk_ctx();
        add(
            &ctx,
            "ops/alice",
            false,
            Some(AGE_KEY_1),
            Some("Alice from ops".into()),
        )
        .unwrap();
        let rdir = rstore::recipients_dir(&ctx.store);
        assert!(rdir.join("ops").join("alice.pub").exists());
        assert!(rdir.join("ops").join("alice.yaml").exists());
    }

    #[test]
    fn validate_name_allows_slashes() {
        assert!(validate_name("ops/alice").is_ok());
        assert!(validate_name("team/sub/bob").is_ok());
    }

    #[test]
    fn validate_name_rejects_invalid() {
        assert!(validate_name("").is_err());
        assert!(validate_name("/leading").is_err());
        assert!(validate_name("trailing/").is_err());
        assert!(validate_name("a//b").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("a/..").is_err());
        assert!(validate_name("a\\.pub").is_err());
    }

    #[test]
    fn recipient_table_highlights_self_only_when_colored() {
        let rows = vec![
            RecipientRow {
                name: "alice".into(),
                description: String::new(),
                key: AGE_KEY_1.into(),
                short_key: short_fingerprint(AGE_KEY_1),
            },
            RecipientRow {
                name: "bot".into(),
                description: String::new(),
                key: AGE_KEY_2.into(),
                short_key: short_fingerprint(AGE_KEY_2),
            },
        ];

        let plain = render_recipient_table(&rows, Some(AGE_KEY_1), false);
        assert!(!plain.contains('\u{1b}'));
        assert!(plain.contains("alice"));
        assert!(plain.contains("bot"));

        let colored = render_recipient_table(&rows, Some(AGE_KEY_1), true);
        assert!(colored.contains('\u{1b}'));
        assert!(colored.contains("alice"));
        assert!(colored.contains("bot"));
    }
}
