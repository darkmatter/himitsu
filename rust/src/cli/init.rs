use std::path::PathBuf;

use clap::Args;

use super::Context;
use crate::config;
use crate::config::Config;
use crate::crypto::age;
use crate::error::Result;

/// Initialize himitsu in the current project (or globally).
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Output result as JSON (for TUI consumption).
    #[arg(long, hide = true)]
    pub json: bool,
}

pub fn run(args: InitArgs, ctx: &Context) -> Result<()> {
    let user_home = &ctx.user_home;
    let store = &ctx.store;
    let in_git_repo = config::find_git_root(&std::env::current_dir()?).is_some();

    // ── 1. Ensure user-level home exists (keys, config, index) ──
    let home_existed = user_home.join("keys/age.txt").exists();

    for dir in &["keys", "state", "cache"] {
        std::fs::create_dir_all(user_home.join(dir))?;
    }

    let key_path = config::key_path(user_home);
    let pubkey = if !key_path.exists() {
        let (secret, public) = age::keygen();
        std::fs::write(
            &key_path,
            format!(
                "# created: {}\n# public key: {public}\n{secret}\n",
                timestamp()
            ),
        )?;
        public
    } else {
        extract_public_key(&std::fs::read_to_string(&key_path)?).unwrap_or_default()
    };

    let config_path = user_home.join(".himitsu.yaml");
    if !config_path.exists() {
        Config::write_default(&config_path)?;
    }

    // ── 2. Ensure project store exists ──
    let store_existed = store.join("vars").exists();

    for dir in &["vars", "recipients/common"] {
        std::fs::create_dir_all(store.join(dir))?;
    }

    let data_json = store.join("data.json");
    if !data_json.exists() {
        std::fs::write(&data_json, "{\"groups\":[\"common\"]}\n")?;
    }

    // Add self as recipient if none exist
    let self_pub = store.join("recipients/common/self.pub");
    if !self_pub.exists() {
        std::fs::write(&self_pub, format!("{pubkey}\n"))?;
    }

    // ── 3. Register this store in the global index ──
    let _ = config::register_store(user_home, store);

    // ── 4. Detect git context for suggestions ──
    let git_root = std::env::current_dir()
        .ok()
        .and_then(|cwd| config::find_git_root(&cwd));
    let suggested_remote = git_root.as_ref().and_then(detect_origin_remote);

    // ── 5. Output ──
    if args.json {
        let json = serde_json::json!({
            "user_home": user_home.to_string_lossy(),
            "store": store.to_string_lossy(),
            "pubkey": pubkey,
            "home_existed": home_existed,
            "store_existed": store_existed,
            "in_git_repo": in_git_repo,
            "suggested_remote": suggested_remote,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if store_existed && home_existed {
        println!("Store: {}", store.display());
        println!("Key:   {pubkey}");
    } else {
        if !home_existed {
            println!("Created keyring at {}", user_home.display());
            println!("Age key: {pubkey}");
        }
        if !store_existed {
            println!("Initialized store at {}", store.display());
            if let Some(ref suggested) = suggested_remote {
                println!("Detected git origin: {suggested}");
            }
        }
    }

    Ok(())
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
