use clap::Args;

use super::Context;
use crate::config::global::GlobalConfig;
use crate::crypto::age;
use crate::error::Result;

/// Initialize a new himitsu store at ~/.himitsu.
#[derive(Debug, Args)]
pub struct InitArgs {}

pub fn run(_args: InitArgs, ctx: &Context) -> Result<()> {
    let home = &ctx.himitsu_home;

    // Create directory structure
    let dirs = ["keys", "data", "cache", "locks", "state"];
    for dir in &dirs {
        std::fs::create_dir_all(home.join(dir))?;
    }

    // Generate age keypair if it doesn't exist
    let key_path = home.join("keys/age.txt");
    if !key_path.exists() {
        let (secret, public) = age::keygen();
        std::fs::write(
            &key_path,
            format!(
                "# created: {}\n# public key: {public}\n{secret}\n",
                chrono_now()
            ),
        )?;
        println!("Generated age key: {public}");
    } else {
        println!("Age key already exists, skipping keygen.");
    }

    // Write default config if it doesn't exist
    let config_path = home.join("config.yaml");
    if !config_path.exists() {
        GlobalConfig::write_default(&config_path)?;
        println!("Created {}", config_path.display());
    }

    println!("Initialized himitsu at {}", home.display());
    Ok(())
}

fn chrono_now() -> String {
    // Simple ISO 8601 timestamp without chrono dependency
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Good enough for a comment timestamp
    format!("{secs}")
}
