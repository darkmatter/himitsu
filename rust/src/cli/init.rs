use std::path::{Path, PathBuf};

use clap::Args;

use super::Context;
use crate::config;
use crate::crypto::age;
use crate::error::Result;

/// Initialize himitsu (create keys, config, and optionally a store).
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Output result as JSON (for TUI consumption).
    #[arg(long, hide = true)]
    pub json: bool,

    /// Register an initial store by slug (e.g. `org/repo`) and set it as the
    /// default. The store is created at `stores_dir/<org>/<repo>` if it does
    /// not already exist.
    #[arg(long)]
    pub name: Option<String>,
}

pub fn run(args: InitArgs, ctx: &Context) -> Result<()> {
    let data_dir = &ctx.data_dir;
    let state_dir = &ctx.state_dir;

    // ── 1. Ensure data_dir exists (keys, config) ──────────────────────────
    let key_existed = data_dir.join("key").exists();

    std::fs::create_dir_all(data_dir)?;

    let key_path = data_dir.join("key");
    let pubkey_path = data_dir.join("key.pub");

    let pubkey = if !key_path.exists() {
        let (secret, public) = age::keygen();
        std::fs::write(
            &key_path,
            format!(
                "# created: {}\n# public key: {public}\n{secret}\n",
                timestamp()
            ),
        )?;
        std::fs::write(&pubkey_path, format!("{public}\n"))?;
        public
    } else {
        read_public_key(data_dir)?
    };

    let config_path = data_dir.join("config.yaml");
    if !config_path.exists() {
        crate::config::Config::write_default(&config_path)?;
    }

    // ── 2. Ensure state_dir exists (stores subdir) ────────────────────────
    std::fs::create_dir_all(state_dir.join("stores"))?;

    // ── 3. Optionally initialize a path-based store (--store flag) ────────
    let store = &ctx.store;
    let store_existed = if store.as_os_str().is_empty() {
        true // no store requested
    } else {
        store_exists(store)
    };

    if !store.as_os_str().is_empty() {
        ensure_store_layout(store, &pubkey)?;
    }

    // ── 4. Handle --name: register a named remote store and set as default ─
    let name_registered = if let Some(ref slug) = args.name {
        let (org, repo) = config::validate_remote_slug(slug)?;
        let dest = config::store_checkout(org, repo);
        if !dest.exists() {
            // Create a fresh local store at stores_dir/<org>/<repo>
            std::fs::create_dir_all(crate::remote::store::secrets_dir(&dest))?;
            let common_dir = crate::remote::store::recipients_dir(&dest).join("common");
            std::fs::create_dir_all(&common_dir)?;
            if !pubkey.is_empty() {
                std::fs::write(common_dir.join("self.pub"), format!("{pubkey}\n"))?;
            }
        }
        // Set (or update) default_store in global config
        let mut cfg = config::Config::load(&config_path)?;
        cfg.default_store = Some(slug.clone());
        cfg.save(&config_path)?;
        true
    } else {
        false
    };

    // ── 5. Detect git context for suggestions ─────────────────────────────
    let in_git_repo = config::find_git_root(&std::env::current_dir()?).is_some();
    let git_root = std::env::current_dir()
        .ok()
        .and_then(|cwd| config::find_git_root(&cwd));
    let suggested_remote = git_root.as_ref().and_then(detect_origin_remote);

    // ── 6. Output ──────────────────────────────────────────────────────────
    // Determine whether anything new was actually created this invocation.
    let anything_created =
        !key_existed || (!store_existed && !store.as_os_str().is_empty()) || name_registered;

    if args.json {
        let json = serde_json::json!({
            "data_dir": data_dir.to_string_lossy(),
            "state_dir": state_dir.to_string_lossy(),
            "store": store.to_string_lossy(),
            "pubkey": pubkey,
            "key_existed": key_existed,
            "store_existed": store_existed,
            "in_git_repo": in_git_repo,
            "suggested_remote": suggested_remote,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if !anything_created {
        // Already fully initialized — show summary.
        println!("Already initialized.");
        println!("  Public key: {pubkey}");
        // Show registered remote stores (if any).
        let remotes = crate::remote::list_remotes().unwrap_or_default();
        if !remotes.is_empty() {
            let cfg = config::Config::load(&config_path)?;
            let default_slug = cfg.default_store.as_deref().unwrap_or("");
            for r in &remotes {
                if r == default_slug {
                    println!("  Stores: {r} (default)");
                } else {
                    println!("  Stores: {r}");
                }
            }
        }
    } else {
        // Wizard summary: show what was created.
        if !key_existed {
            println!("✓ Created age keypair");
            println!("  Public key: {pubkey}");
        }
        if !store_existed && !store.as_os_str().is_empty() {
            println!("✓ Initialized store at {}", store.display());
            if let Some(ref suggested) = suggested_remote {
                println!("  Detected git origin: {suggested}");
            }
        }
        if name_registered {
            let slug = args.name.as_deref().unwrap_or("");
            println!("✓ Registered store {slug} (default)");
        }
        println!("✓ Created state directory");

        // Prompt to add a remote store if none was set up.
        if store.as_os_str().is_empty() && !name_registered {
            println!();
            println!("Run `himitsu remote add <org/repo>` to add a secret store.");
        }
    }

    Ok(())
}

pub(crate) fn read_public_key(data_dir: &Path) -> Result<String> {
    let pubkey_path = data_dir.join("key.pub");
    if pubkey_path.exists() {
        return Ok(std::fs::read_to_string(&pubkey_path)?.trim().to_string());
    }

    let key_path = data_dir.join("key");
    let contents = std::fs::read_to_string(&key_path)?;
    Ok(extract_public_key(&contents).unwrap_or_default())
}

pub(crate) fn store_exists(store: &Path) -> bool {
    crate::remote::store::secrets_dir(store).exists()
}

pub(crate) fn ensure_store_layout(store: &Path, pubkey: &str) -> Result<bool> {
    let existed = store_exists(store);

    if !existed {
        std::fs::create_dir_all(crate::remote::store::secrets_dir(store))?;
    }

    let self_pub = crate::remote::store::recipients_dir(store)
        .join("common")
        .join("self.pub");
    if !self_pub.exists() && !pubkey.is_empty() {
        std::fs::create_dir_all(self_pub.parent().unwrap())?;
        std::fs::write(&self_pub, format!("{pubkey}\n"))?;
    }

    Ok(!existed)
}

fn timestamp() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

fn extract_public_key(contents: &str) -> Option<String> {
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("# public key: ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn detect_origin_remote(git_root: &PathBuf) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(git_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_remote_slug(&url)
}

fn parse_remote_slug(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return Some(rest.strip_suffix(".git").unwrap_or(rest).to_string());
    }
    if let Some(rest) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        return Some(rest.strip_suffix(".git").unwrap_or(rest).to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssh_remote() {
        assert_eq!(
            parse_remote_slug("git@github.com:myorg/myrepo.git"),
            Some("myorg/myrepo".into())
        );
    }

    #[test]
    fn parse_https_remote() {
        assert_eq!(
            parse_remote_slug("https://github.com/myorg/myrepo.git"),
            Some("myorg/myrepo".into())
        );
    }

    #[test]
    fn parse_unknown_url_returns_none() {
        assert_eq!(parse_remote_slug("https://gitlab.com/foo/bar"), None);
    }
}
