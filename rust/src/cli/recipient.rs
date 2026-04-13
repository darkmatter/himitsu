use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
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
    /// Recipients live in a flat layout: `<store>/.himitsu/recipients/<name>.pub`.
    /// Group membership is managed separately via `himitsu group add-recipient`.
    ///
    /// Examples:
    ///   himitsu recipient add laptop --self
    ///   himitsu recipient add alice --age-key age1... --description "Alice"
    Add {
        /// Recipient name (e.g. laptop-a, alice).
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
    /// Remove a recipient (deletes pub + sidecar and strips from every group).
    Rm {
        /// Name of the recipient to remove.
        name: String,
        /// (Deprecated) Group to remove the recipient from. Kept for CLI
        /// backwards compatibility — now removes membership in that group
        /// only, leaving the `.pub` file and other memberships intact.
        #[arg(long)]
        group: Option<String>,
    },
    /// Show a recipient's key, description and groups.
    Show {
        /// Recipient name to look up.
        name: String,
        /// (Deprecated) Accepted for backwards compatibility; ignored.
        #[arg(long, hide = true)]
        group: Option<String>,
    },
    /// List recipients in a plain aligned table.
    Ls {
        /// Filter to members of a specific group.
        #[arg(long)]
        group: Option<String>,
    },
}

/// Sidecar metadata stored beside each `<name>.pub`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecipientMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
}

pub fn run(args: RecipientArgs, ctx: &Context) -> Result<()> {
    migrate_legacy_layout(ctx)?;

    match args.command {
        RecipientCommand::Add {
            name,
            self_,
            age_key,
            description,
        } => add(ctx, &name, self_, age_key.as_deref(), description),

        RecipientCommand::Rm { name, group } => rm(ctx, &name, group.as_deref()),

        RecipientCommand::Show { name, group: _ } => show(ctx, &name),

        RecipientCommand::Ls { group } => ls(ctx, group.as_deref()),
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

    ctx.commit_and_push(&format!("himitsu: add recipient {name}"));
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

fn rm(ctx: &Context, name: &str, group: Option<&str>) -> Result<()> {
    let recipients_dir = flat_recipients_dir(ctx);

    // `--group` is deprecated: only drop membership in that group, keep files.
    if let Some(group_name) = group {
        eprintln!(
            "warning: `recipient rm --group` is deprecated. \
             Use `group rm-recipient {group_name} {name}` instead."
        );
        let mut cfg = rstore::load_store_config(&ctx.store)?;
        let changed = match cfg.recipients.groups.get_mut(group_name) {
            Some(members) => {
                let before = members.len();
                members.retain(|m| m != name);
                members.len() != before
            }
            None => false,
        };
        if !changed {
            return Err(HimitsuError::Recipient(format!(
                "recipient '{name}' is not a member of group '{group_name}'"
            )));
        }
        rstore::save_store_config(&ctx.store, &cfg)?;
        ctx.commit_and_push(&format!(
            "himitsu: remove recipient {name} from group {group_name}"
        ));
        println!("Removed recipient '{name}' from group '{group_name}'");
        return Ok(());
    }

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

    // Strip from every group in the store config.
    let mut cfg = rstore::load_store_config(&ctx.store)?;
    let mut touched = false;
    for members in cfg.recipients.groups.values_mut() {
        let before = members.len();
        members.retain(|m| m != name);
        if members.len() != before {
            touched = true;
        }
    }
    if touched {
        rstore::save_store_config(&ctx.store, &cfg)?;
    }

    ctx.commit_and_push(&format!("himitsu: remove recipient {name}"));
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

    let cfg = rstore::load_store_config(&ctx.store)?;
    let groups = groups_for(&cfg, name);
    if groups.is_empty() {
        println!("Groups:       (none)");
    } else {
        println!("Groups:       {}", groups.join(", "));
    }
    Ok(())
}

// ── ls ──────────────────────────────────────────────────────────────────────

fn ls(ctx: &Context, group_filter: Option<&str>) -> Result<()> {
    let recipients_dir = flat_recipients_dir(ctx);
    if !recipients_dir.exists() {
        return Ok(());
    }
    let cfg = rstore::load_store_config(&ctx.store)?;

    let filter_members: Option<Vec<String>> = group_filter.map(|g| {
        cfg.recipients
            .groups
            .get(g)
            .cloned()
            .unwrap_or_default()
    });

    let mut rows: Vec<(String, String, String, String)> = vec![];
    for entry in std::fs::read_dir(&recipients_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy().to_string();
        let Some(name) = fname.strip_suffix(".pub") else {
            continue;
        };
        if let Some(ref members) = filter_members {
            if !members.iter().any(|m| m == name) {
                continue;
            }
        }
        let key = std::fs::read_to_string(entry.path())?
            .trim()
            .to_string();
        let meta = read_sidecar(&recipients_dir, name);
        let description = meta.description.unwrap_or_default();
        let groups = groups_for(&cfg, name).join(",");
        let short_key = short_fingerprint(&key);
        rows.push((name.to_string(), description, groups, short_key));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    if rows.is_empty() {
        return Ok(());
    }

    let headers = ("NAME", "DESCRIPTION", "GROUPS", "KEY");
    let w0 = rows.iter().map(|r| r.0.len()).max().unwrap_or(0).max(headers.0.len());
    let w1 = rows.iter().map(|r| r.1.len()).max().unwrap_or(0).max(headers.1.len());
    let w2 = rows.iter().map(|r| r.2.len()).max().unwrap_or(0).max(headers.2.len());
    println!(
        "{:<w0$}  {:<w1$}  {:<w2$}  {}",
        headers.0,
        headers.1,
        headers.2,
        headers.3,
        w0 = w0,
        w1 = w1,
        w2 = w2
    );
    for r in &rows {
        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}",
            r.0,
            r.1,
            r.2,
            r.3,
            w0 = w0,
            w1 = w1,
            w2 = w2
        );
    }
    Ok(())
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

fn groups_for(cfg: &rstore::StoreFileConfig, name: &str) -> Vec<String> {
    let mut out: Vec<String> = cfg
        .recipients
        .groups
        .iter()
        .filter_map(|(g, members)| {
            if members.iter().any(|m| m == name) {
                Some(g.clone())
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
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
        || name.contains('/')
        || name.contains('\\')
        || name.starts_with('.')
        || name.contains("..")
    {
        return Err(HimitsuError::Recipient(format!(
            "invalid recipient name '{name}'"
        )));
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

// ── migration ───────────────────────────────────────────────────────────────

/// Migrate a legacy `<group>/<name>.pub` layout to the flat layout.
///
/// - Walks every subdirectory under `.himitsu/recipients/`.
/// - Moves each `<group>/<name>.pub` to `<name>.pub` (last-group-wins on
///   name collisions, with the earlier key preserved as a `.pub.<group>` backup).
/// - Builds the `recipients.groups` mapping in `.himitsu/config.yaml`.
/// - Commits via `ctx.commit_and_push`.
///
/// Idempotent — does nothing if no subdirectories are present.
pub fn migrate_legacy_layout(ctx: &Context) -> Result<()> {
    if ctx.store.as_os_str().is_empty() {
        return Ok(());
    }
    let recipients_dir = flat_recipients_dir(ctx);
    if !recipients_dir.exists() {
        return Ok(());
    }

    // Detect any group subdirectories.
    let mut group_dirs: Vec<(String, PathBuf)> = vec![];
    for entry in std::fs::read_dir(&recipients_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let gname = entry.file_name().to_string_lossy().to_string();
            group_dirs.push((gname, entry.path()));
        }
    }
    if group_dirs.is_empty() {
        return Ok(());
    }
    group_dirs.sort_by(|a, b| a.0.cmp(&b.0));

    let mut cfg = rstore::load_store_config(&ctx.store)?;

    for (group_name, dir) in &group_dirs {
        let mut members: Vec<String> = vec![];
        for file in std::fs::read_dir(dir)? {
            let file = file?;
            if !file.file_type()?.is_file() {
                continue;
            }
            let fname = file.file_name().to_string_lossy().to_string();
            let Some(name) = fname.strip_suffix(".pub") else {
                continue;
            };
            let src = file.path();
            let contents = std::fs::read_to_string(&src)?;
            let dst = recipients_dir.join(format!("{name}.pub"));
            if dst.exists() {
                let existing = std::fs::read_to_string(&dst)?;
                if existing.trim() != contents.trim() {
                    // Preserve divergent key under a backup name.
                    let backup =
                        recipients_dir.join(format!("{name}.pub.{group_name}"));
                    std::fs::write(&backup, contents)?;
                }
            } else {
                std::fs::write(&dst, contents)?;
            }
            std::fs::remove_file(&src)?;
            if !members.contains(&name.to_string()) {
                members.push(name.to_string());
            }
        }
        // Remove the now-empty group dir; ignore if not actually empty.
        let _ = std::fs::remove_dir(dir);
        if !members.is_empty() {
            let entry = cfg
                .recipients
                .groups
                .entry(group_name.clone())
                .or_default();
            for m in members {
                if !entry.contains(&m) {
                    entry.push(m);
                }
            }
        }
    }

    rstore::save_store_config(&ctx.store, &cfg)?;
    ctx.commit_and_push("himitsu: migrate recipients to flat layout");
    Ok(())
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
        };
        (tmp, ctx)
    }

    const AGE_KEY_1: &str =
        "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";
    const AGE_KEY_2: &str =
        "age1lvyvwawkr0mcnnnncaghunadrqkmuf9e6507x9y920xxpp866cnql7dp2z";

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
    fn rm_deletes_pub_and_sidecar_and_strips_groups() {
        let (_tmp, ctx) = mk_ctx();
        add(&ctx, "alice", false, Some(AGE_KEY_1), Some("desc".into())).unwrap();
        // Add alice to two groups.
        let mut cfg = rstore::load_store_config(&ctx.store).unwrap();
        cfg.recipients
            .groups
            .insert("common".into(), vec!["alice".into()]);
        cfg.recipients
            .groups
            .insert("admins".into(), vec!["alice".into(), "bob".into()]);
        rstore::save_store_config(&ctx.store, &cfg).unwrap();

        rm(&ctx, "alice", None).unwrap();

        let rdir = rstore::recipients_dir(&ctx.store);
        assert!(!rdir.join("alice.pub").exists());
        assert!(!rdir.join("alice.yaml").exists());
        let after = rstore::load_store_config(&ctx.store).unwrap();
        assert!(after.recipients.groups["common"].is_empty());
        assert_eq!(after.recipients.groups["admins"], vec!["bob"]);
    }

    #[test]
    fn migration_converts_legacy_layout() {
        let (_tmp, ctx) = mk_ctx();
        // Seed a legacy layout.
        let rdir = rstore::recipients_dir(&ctx.store);
        std::fs::create_dir_all(rdir.join("common")).unwrap();
        std::fs::create_dir_all(rdir.join("admins")).unwrap();
        std::fs::write(rdir.join("common").join("alice.pub"), format!("{AGE_KEY_1}\n"))
            .unwrap();
        std::fs::write(rdir.join("common").join("bob.pub"), format!("{AGE_KEY_2}\n"))
            .unwrap();
        std::fs::write(rdir.join("admins").join("alice.pub"), format!("{AGE_KEY_1}\n"))
            .unwrap();

        migrate_legacy_layout(&ctx).unwrap();

        assert!(rdir.join("alice.pub").exists());
        assert!(rdir.join("bob.pub").exists());
        assert!(!rdir.join("common").exists());
        assert!(!rdir.join("admins").exists());

        let cfg = rstore::load_store_config(&ctx.store).unwrap();
        assert_eq!(cfg.recipients.groups["common"], vec!["alice", "bob"]);
        assert_eq!(cfg.recipients.groups["admins"], vec!["alice"]);
    }

    #[test]
    fn migration_is_idempotent_on_flat_layout() {
        let (_tmp, ctx) = mk_ctx();
        add(&ctx, "alice", false, Some(AGE_KEY_1), None).unwrap();
        let before = std::fs::read_to_string(rstore::store_config_path(&ctx.store))
            .unwrap_or_default();
        migrate_legacy_layout(&ctx).unwrap();
        migrate_legacy_layout(&ctx).unwrap();
        let after = std::fs::read_to_string(rstore::store_config_path(&ctx.store))
            .unwrap_or_default();
        assert_eq!(before, after);
        assert!(rstore::recipients_dir(&ctx.store).join("alice.pub").exists());
    }
}
