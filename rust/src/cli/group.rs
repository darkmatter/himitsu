use clap::{Args, Subcommand};

use super::Context;
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
    Add { name: String },
    /// Remove a group.
    Rm { name: String },
    /// List all groups.
    Ls,
}

pub fn run(args: GroupArgs, ctx: &Context) -> Result<()> {
    let recipients_dir = ctx.store.join("recipients");
    let data_json_path = ctx.store.join("data.json");

    match args.command {
        GroupCommand::Add { name } => {
            std::fs::create_dir_all(recipients_dir.join(&name))?;
            update_data_json(&data_json_path, |groups| {
                if !groups.contains(&name) {
                    groups.push(name.clone());
                    groups.sort();
                }
            })?;
            ctx.commit_and_push(&format!("himitsu: add group {name}"));
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
            update_data_json(&data_json_path, |groups| {
                groups.retain(|g| g != &name);
            })?;
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
