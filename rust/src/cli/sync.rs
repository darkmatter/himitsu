use std::path::{Path, PathBuf};

use clap::Args;
use serde::{Deserialize, Serialize};

use super::Context;
use crate::config;
use crate::error::{HimitsuError, Result};
use crate::remote::store;

/// Sync: mirror encrypted files from a bound remote into the current store.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Path prefix to sync. If omitted, syncs all secrets.
    pub env: Option<String>,

    /// Bind this store to a remote for future syncs (e.g., `org/repo`).
    #[arg(long)]
    pub bind: Option<String>,
}

/// Persisted binding stored at `<store>/remote.yaml`.
#[derive(Debug, Serialize, Deserialize)]
struct RemoteBinding {
    remote: String,
}

fn binding_path(store: &Path) -> PathBuf {
    store.join("remote.yaml")
}

fn save_binding(store: &Path, slug: &str) -> Result<()> {
    let binding = RemoteBinding {
        remote: slug.to_string(),
    };
    let yaml = serde_yaml::to_string(&binding)?;
    std::fs::create_dir_all(store)?;
    std::fs::write(binding_path(store), yaml)?;
    Ok(())
}

fn load_binding(store: &Path) -> Result<String> {
    let path = binding_path(store);
    if !path.exists() {
        return Err(HimitsuError::InvalidConfig(
            "no remote binding found; run `himitsu sync --bind <org/repo>` first".into(),
        ));
    }
    let contents = std::fs::read_to_string(&path)?;
    let binding: RemoteBinding = serde_yaml::from_str(&contents)?;
    if binding.remote.is_empty() {
        return Err(HimitsuError::InvalidConfig(
            "remote.yaml binding has an empty remote slug".into(),
        ));
    }
    Ok(binding.remote)
}

/// Recursively copy all files from `src_dir` into `dst_dir`, preserving structure.
/// Returns the number of files copied.
fn copy_tree(src_dir: &Path, dst_dir: &Path) -> Result<usize> {
    if !src_dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in std::fs::read_dir(src_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src = entry.path();
        let dst = dst_dir.join(entry.file_name());
        if file_type.is_dir() {
            std::fs::create_dir_all(&dst)?;
            count += copy_tree(&src, &dst)?;
        } else if file_type.is_file() {
            std::fs::create_dir_all(dst_dir)?;
            std::fs::copy(&src, &dst)?;
            count += 1;
        }
    }
    Ok(count)
}

pub fn run(args: SyncArgs, ctx: &Context) -> Result<()> {
    // ── bind subcommand ──────────────────────────────────────────────────────
    if let Some(slug) = &args.bind {
        config::validate_remote_slug(slug)?;
        // Verify the remote actually exists locally before persisting the binding.
        config::remote_store_path(slug)?;
        save_binding(&ctx.store, slug)?;
        println!("Bound store to remote {slug}");
        return Ok(());
    }

    // ── mirror sync ──────────────────────────────────────────────────────────
    let slug = load_binding(&ctx.store)?;
    let remote_path = config::remote_store_path(&slug)?;

    // Best-effort git pull on the remote.
    match crate::git::pull(&remote_path) {
        Ok(_) => tracing::debug!("pulled remote {slug}"),
        Err(e) => tracing::debug!("git pull skipped for {slug}: {e}"),
    }

    // Mirror secrets (optionally filtered by path prefix).
    let src_secrets = store::secrets_dir(&remote_path);
    let dst_secrets = store::secrets_dir(&ctx.store);

    let secrets_count = if let Some(ref prefix) = args.env {
        let src = src_secrets.join(prefix);
        let dst = dst_secrets.join(prefix);
        copy_tree(&src, &dst)?
    } else {
        copy_tree(&src_secrets, &dst_secrets)?
    };

    // Mirror recipient public-key material.
    let recipients_count = copy_tree(
        &store::recipients_dir(&remote_path),
        &store::recipients_dir(&ctx.store),
    )?;

    ctx.commit_and_push(&format!(
        "himitsu: sync {secrets_count} secret(s) from {slug}"
    ));

    println!(
        "Synced {secrets_count} secret(s) and {recipients_count} recipient file(s) from {slug}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_binding_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        save_binding(tmp.path(), "my-org/my-repo").unwrap();
        let slug = load_binding(tmp.path()).unwrap();
        assert_eq!(slug, "my-org/my-repo");
    }

    #[test]
    fn load_binding_errors_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load_binding(tmp.path()).unwrap_err();
        assert!(matches!(err, HimitsuError::InvalidConfig(_)));
    }

    #[test]
    fn copy_tree_copies_files_recursively() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(src.path().join("sub")).unwrap();
        std::fs::write(src.path().join("a.age"), b"data1").unwrap();
        std::fs::write(src.path().join("sub/b.age"), b"data2").unwrap();

        let count = copy_tree(src.path(), dst.path()).unwrap();
        assert_eq!(count, 2);
        assert_eq!(std::fs::read(dst.path().join("a.age")).unwrap(), b"data1");
        assert_eq!(
            std::fs::read(dst.path().join("sub/b.age")).unwrap(),
            b"data2"
        );
    }

    #[test]
    fn copy_tree_returns_zero_for_missing_src() {
        let tmp = tempfile::tempdir().unwrap();
        let count = copy_tree(&tmp.path().join("nonexistent"), tmp.path()).unwrap();
        assert_eq!(count, 0);
    }
}
