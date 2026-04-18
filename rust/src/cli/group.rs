use clap::{Args, Subcommand};

use super::Context;
use crate::error::{HimitsuError, Result};
use crate::remote::store as rstore;

/// Manage recipient groups (deprecated — use path-based recipient names instead).
///
/// Groups are deprecated. Instead, use path-based recipient names:
///   himitsu recipient add ops/alice --age-key age1...
///   himitsu recipient add ops/bob --age-key age1...
///
/// Then reference all ops recipients with `ops/*` in encryption configs.
#[derive(Debug, Args)]
pub struct GroupArgs {
    #[command(subcommand)]
    pub command: GroupCommand,
}

#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// Create an empty group.
    Add { name: String },
    /// Remove a group (not its member recipients).
    Rm { name: String },
    /// List groups with member counts.
    Ls,
    /// Add a recipient to a group.
    AddRecipient {
        /// Group name.
        group: String,
        /// Recipient name (must already exist under `recipients/<name>.pub`).
        recipient: String,
    },
    /// Remove a recipient from a group.
    RmRecipient {
        /// Group name.
        group: String,
        /// Recipient name to drop from `group`.
        recipient: String,
    },
}

pub fn run(args: GroupArgs, ctx: &Context) -> Result<()> {
    eprintln!(
        "warning: `himitsu group` is deprecated. Use path-based recipient names instead.\n\
         Example: `himitsu recipient add ops/alice --age-key age1...`\n\
         Then reference all ops recipients with `ops/*` in encryption configs.\n"
    );
    super::recipient::migrate_legacy_layout(ctx)?;
    let mut cfg = rstore::load_store_config(&ctx.store)?;

    match args.command {
        GroupCommand::Add { name } => {
            if name.is_empty() || name.contains('/') {
                return Err(HimitsuError::Group(format!("invalid group name '{name}'")));
            }
            if cfg.recipients.groups.contains_key(&name) {
                return Err(HimitsuError::Group(format!(
                    "group '{name}' already exists"
                )));
            }
            cfg.recipients.groups.insert(name.clone(), vec![]);
            rstore::save_store_config(&ctx.store, &cfg)?;
            println!("Created group '{name}'");
        }

        GroupCommand::Rm { name } => {
            if name == "common" {
                return Err(HimitsuError::Group(
                    "cannot remove reserved group 'common' — it is the default group for 'recipient add' and 'init --self'".into(),
                ));
            }
            if cfg.recipients.groups.remove(&name).is_none() {
                return Err(HimitsuError::Group(format!(
                    "group '{name}' does not exist"
                )));
            }
            rstore::save_store_config(&ctx.store, &cfg)?;
            println!("Removed group '{name}'");
        }

        GroupCommand::Ls => {
            let mut groups: Vec<(&String, usize)> = cfg
                .recipients
                .groups
                .iter()
                .map(|(k, v)| (k, v.len()))
                .collect();
            groups.sort_by(|a, b| a.0.cmp(b.0));
            for (name, count) in &groups {
                println!("{name}\t{count} recipient(s)");
            }
        }

        GroupCommand::AddRecipient { group, recipient } => {
            let pub_file = rstore::recipients_dir_with_override(
                &ctx.store,
                ctx.recipients_path.as_deref(),
            )
            .join(format!("{recipient}.pub"));
            if !pub_file.exists() {
                return Err(HimitsuError::Recipient(format!(
                    "recipient '{recipient}' not found (add it first with `recipient add`)"
                )));
            }
            let members = cfg.recipients.groups.entry(group.clone()).or_default();
            if members.iter().any(|m| m == &recipient) {
                return Err(HimitsuError::Group(format!(
                    "recipient '{recipient}' is already a member of '{group}'"
                )));
            }
            members.push(recipient.clone());
            rstore::save_store_config(&ctx.store, &cfg)?;
            println!("Added '{recipient}' to group '{group}'");
        }

        GroupCommand::RmRecipient { group, recipient } => {
            let Some(members) = cfg.recipients.groups.get_mut(&group) else {
                return Err(HimitsuError::Group(format!(
                    "group '{group}' does not exist"
                )));
            };
            let before = members.len();
            members.retain(|m| m != &recipient);
            if members.len() == before {
                return Err(HimitsuError::Group(format!(
                    "recipient '{recipient}' is not a member of '{group}'"
                )));
            }
            rstore::save_store_config(&ctx.store, &cfg)?;
            println!("Removed '{recipient}' from group '{group}'");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const AGE_KEY: &str =
        "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";

    fn mk_ctx() -> (TempDir, Context) {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().to_path_buf();
        std::fs::create_dir_all(rstore::secrets_dir(&store)).unwrap();
        let rdir = rstore::recipients_dir(&store);
        std::fs::create_dir_all(&rdir).unwrap();
        std::fs::write(rdir.join("alice.pub"), format!("{AGE_KEY}\n")).unwrap();
        let ctx = Context {
            data_dir: tmp.path().join("data"),
            state_dir: tmp.path().join("state"),
            store,
            recipients_path: None,
        };
        (tmp, ctx)
    }

    #[test]
    fn add_recipient_and_rm_recipient_round_trip() {
        let (_tmp, ctx) = mk_ctx();
        run(
            GroupArgs {
                command: GroupCommand::AddRecipient {
                    group: "admins".into(),
                    recipient: "alice".into(),
                },
            },
            &ctx,
        )
        .unwrap();
        let cfg = rstore::load_store_config(&ctx.store).unwrap();
        assert_eq!(cfg.recipients.groups["admins"], vec!["alice"]);

        run(
            GroupArgs {
                command: GroupCommand::RmRecipient {
                    group: "admins".into(),
                    recipient: "alice".into(),
                },
            },
            &ctx,
        )
        .unwrap();
        let cfg = rstore::load_store_config(&ctx.store).unwrap();
        assert!(cfg.recipients.groups["admins"].is_empty());
    }

    #[test]
    fn add_group_is_idempotent_failure() {
        let (_tmp, ctx) = mk_ctx();
        run(
            GroupArgs {
                command: GroupCommand::Add {
                    name: "ops".into(),
                },
            },
            &ctx,
        )
        .unwrap();
        let err = run(
            GroupArgs {
                command: GroupCommand::Add {
                    name: "ops".into(),
                },
            },
            &ctx,
        );
        assert!(err.is_err());
    }

    #[test]
    fn add_recipient_unknown_errors() {
        let (_tmp, ctx) = mk_ctx();
        let err = run(
            GroupArgs {
                command: GroupCommand::AddRecipient {
                    group: "ops".into(),
                    recipient: "ghost".into(),
                },
            },
            &ctx,
        );
        assert!(err.is_err());
    }
}
