use clap::{Args, Subcommand};

use super::Context;
use crate::config;
use crate::error::{HimitsuError, Result};

/// Manage recipient groups.
#[derive(Debug, Args)]
pub struct GroupArgs {
    #[command(subcommand)]
    pub command: GroupCommand,
}

#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// Add a new group.
    Add {
        /// Name of the group to create.
        name: String,
    },

    /// Remove a group.
    Rm {
        /// Name of the group to remove.
        name: String,
    },

    /// List all groups.
    Ls,
}

pub fn run(args: GroupArgs, ctx: &Context) -> Result<()> {
    let mode = config::detect_mode(&std::env::current_dir()?);
    let remote_ref = config::resolve_remote(&ctx.remote_override, &mode, &ctx.himitsu_home)?;
    let remote_path = config::remote_path(&ctx.himitsu_home, &remote_ref);
    crate::remote::ensure_remote_exists(&remote_path)?;

    let recipients_dir = remote_path.join("recipients");
    let data_json_path = remote_path.join("data.json");

    match args.command {
        GroupCommand::Add { name } => {
            let group_dir = recipients_dir.join(&name);
            std::fs::create_dir_all(&group_dir)?;

            // Update data.json
            update_data_json(&data_json_path, |groups| {
                if !groups.contains(&name) {
                    groups.push(name.clone());
                    groups.sort();
                }
            })?;

            println!("Created group '{name}'");
        }

        GroupCommand::Rm { name } => {
            if name == "common" {
                return Err(HimitsuError::Group(
                    "cannot remove reserved group 'common'".into(),
                ));
            }

            let group_dir = recipients_dir.join(&name);
            if !group_dir.exists() {
                return Err(HimitsuError::Group(format!(
                    "group '{name}' does not exist"
                )));
            }
            std::fs::remove_dir_all(&group_dir)?;

            // Update data.json
            update_data_json(&data_json_path, |groups| {
                groups.retain(|g| g != &name);
            })?;

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

/// Read data.json, apply a mutation to the groups list, and write it back.
fn update_data_json(path: &std::path::Path, mutate: impl FnOnce(&mut Vec<String>)) -> Result<()> {
    let mut data: serde_json::Value = if path.exists() {
        let contents = std::fs::read_to_string(path)?;
        serde_json::from_str(&contents)?
    } else {
        serde_json::json!({ "groups": [] })
    };

    let groups = data
        .get_mut("groups")
        .and_then(|v| v.as_array_mut())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        });

    let mut group_list = groups.unwrap_or_default();
    mutate(&mut group_list);

    data["groups"] = serde_json::json!(group_list);
    let json = serde_json::to_string_pretty(&data)?;
    std::fs::write(path, format!("{json}\n"))?;
    Ok(())
}
