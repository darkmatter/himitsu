use clap::{Args, Subcommand};

use super::Context;
use crate::error::{HimitsuError, Result};
use crate::remote::store as rstore;

/// Manage recipient groups.
#[derive(Debug, Args)]
pub struct GroupArgs {
    #[command(subcommand)]
    pub command: GroupCommand,
}

#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// Add a new group.
    Add { name: String },
    /// Remove a group.
    Rm { name: String },
    /// List all groups.
    Ls,
}

pub fn run(args: GroupArgs, ctx: &Context) -> Result<()> {
    let recipients_dir =
        rstore::recipients_dir_with_override(&ctx.store, ctx.recipients_path.as_deref());

    match args.command {
        GroupCommand::Add { name } => {
            std::fs::create_dir_all(recipients_dir.join(&name))?;
            ctx.commit_and_push(&format!("himitsu: add group {name}"));
            println!("Created group '{name}'");
        }

        GroupCommand::Rm { name } => {
            if name == "common" {
                return Err(HimitsuError::Group(
                    "cannot remove reserved group 'common' — it is the default group for 'recipient add' and 'init --self'".into(),
                ));
            }
            let group_dir = recipients_dir.join(&name);
            if !group_dir.exists() {
                return Err(HimitsuError::Group(format!(
                    "group '{name}' does not exist"
                )));
            }
            std::fs::remove_dir_all(&group_dir)?;
            ctx.commit_and_push(&format!("himitsu: remove group {name}"));
            println!("Removed group '{name}'");
        }

        GroupCommand::Ls => {
            if !recipients_dir.exists() {
                return Ok(());
            }
            let mut groups: Vec<(String, usize)> = vec![];
            for entry in std::fs::read_dir(&recipients_dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let count = std::fs::read_dir(entry.path())?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".pub"))
                    .count();
                groups.push((name, count));
            }
            groups.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, count) in &groups {
                println!("{name}\t{count} recipient(s)");
            }
        }
    }

    Ok(())
}
