use clap::{Args, Subcommand};

use super::Context;
use crate::config;
use crate::crypto::age;
use crate::error::{HimitsuError, Result};

/// Manage recipients.
#[derive(Debug, Args)]
pub struct RecipientArgs {
    #[command(subcommand)]
    pub command: RecipientCommand,
}

#[derive(Debug, Subcommand)]
pub enum RecipientCommand {
    /// Add a recipient.
    Add {
        /// Recipient name (e.g. laptop-a, alice).
        name: String,
        /// Add yourself as a recipient.
        #[arg(long = "self")]
        self_: bool,
        /// Explicit age public key.
        #[arg(long)]
        age_key: Option<String>,
        /// Group to add the recipient to.
        #[arg(long)]
        group: Option<String>,
    },
    /// Remove a recipient.
    Rm {
        /// Name of the recipient to remove.
        name: String,
        /// Group to remove the recipient from.
        #[arg(long)]
        group: Option<String>,
    },
    /// List recipients.
    Ls {
        /// Filter by group.
        #[arg(long)]
        group: Option<String>,
    },
}

pub fn run(args: RecipientArgs, ctx: &Context) -> Result<()> {
    match args.command {
        RecipientCommand::Add {
            name,
            self_,
            age_key,
            group,
        } => {
            let pubkey = if self_ {
                let key_path = config::key_path(&ctx.user_home);
                let contents = std::fs::read_to_string(&key_path)?;
                extract_public_key(&contents).ok_or_else(|| {
                    HimitsuError::Recipient("cannot extract public key from age.txt".into())
                })?
            } else if let Some(key) = age_key {
                age::parse_recipient(&key)?;
                key
            } else {
                return Err(HimitsuError::Recipient(
                    "either --self or --age-key must be provided".into(),
                ));
            };

            let group_name = group.as_deref().unwrap_or("common");
            let group_dir = ctx.store.join("recipients").join(group_name);
            std::fs::create_dir_all(&group_dir)?;

            let pub_file = group_dir.join(format!("{name}.pub"));
            std::fs::write(&pub_file, format!("{pubkey}\n"))?;
            ctx.commit_and_push(&format!("himitsu: add recipient {name} to {group_name}"));
            println!("Added recipient '{name}' to group '{group_name}'");
        }

        RecipientCommand::Rm { name, group } => {
            let recipients_dir = ctx.store.join("recipients");
            let removed = if let Some(group_name) = &group {
                let pub_file = recipients_dir.join(group_name).join(format!("{name}.pub"));
                if pub_file.exists() {
                    std::fs::remove_file(&pub_file)?;
                    true
                } else {
                    false
                }
            } else {
                let mut found = false;
                if recipients_dir.exists() {
                    for entry in std::fs::read_dir(&recipients_dir)? {
                        let entry = entry?;
                        if entry.file_type()?.is_dir() {
                            let pub_file = entry.path().join(format!("{name}.pub"));
                            if pub_file.exists() {
                                std::fs::remove_file(&pub_file)?;
                                found = true;
                            }
                        }
                    }
                }
                found
            };

            if removed {
                ctx.commit_and_push(&format!("himitsu: remove recipient {name}"));
                println!("Removed recipient '{name}'");
            } else {
                return Err(HimitsuError::Recipient(format!(
                    "recipient '{name}' not found"
                )));
            }
        }

        RecipientCommand::Ls { group } => {
            let recipients_dir = ctx.store.join("recipients");
            if !recipients_dir.exists() {
                return Ok(());
            }
            for entry in std::fs::read_dir(&recipients_dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let group_name = entry.file_name().to_string_lossy().to_string();
                if let Some(ref g) = group {
                    if &group_name != g {
                        continue;
                    }
                }
                for file in std::fs::read_dir(entry.path())? {
                    let file = file?;
                    let fname = file.file_name().to_string_lossy().to_string();
                    if fname.ends_with(".pub") {
                        let name = fname.strip_suffix(".pub").unwrap();
                        let key = std::fs::read_to_string(file.path())?;
                        println!("{group_name}/{name}\t{}", key.trim());
                    }
                }
            }
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
