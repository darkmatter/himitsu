use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Args, Subcommand, ValueEnum};

use crate::config;
use crate::error::{HimitsuError, Result};

/// Manage the GitHub Actions workflow for self-serve himitsu rekeys.
#[derive(Debug, Args)]
pub struct CiArgs {
    #[command(subcommand)]
    pub command: CiCommand,
}

#[derive(Debug, Subcommand)]
pub enum CiCommand {
    /// Show whether the himitsu workflow is installed in this repository.
    Status(WorkflowPathArgs),
    /// Install the himitsu self-serve rekey workflow into .github/workflows/.
    Install(InstallArgs),
    /// Trigger the installed workflow through the GitHub CLI.
    Run(RunArgs),
}

#[derive(Debug, Args)]
pub struct WorkflowPathArgs {
    /// Workflow file path, relative to the current directory by default.
    #[arg(long, default_value = ".github/workflows/himitsu.yml")]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Default remote slug to prefill in the workflow_dispatch form.
    #[arg(long = "default-remote")]
    pub default_remote: Option<String>,

    /// Git ref for the himitsu action used by the generated workflow.
    #[arg(long, default_value = "main")]
    pub action_ref: String,

    /// Workflow file path, relative to the current directory by default.
    #[arg(long, default_value = ".github/workflows/himitsu.yml")]
    pub path: PathBuf,

    /// Overwrite an existing workflow file.
    #[arg(long)]
    pub force: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CiOperation {
    Sync,
    AddRecipient,
    RmRecipient,
}

impl CiOperation {
    fn as_input(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::AddRecipient => "add-recipient",
            Self::RmRecipient => "rm-recipient",
        }
    }
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Operation to trigger in the workflow.
    #[arg(long, value_enum, default_value_t = CiOperation::Sync)]
    pub operation: CiOperation,

    /// Target remote slug (org/repo).
    #[arg(long = "target-remote")]
    pub target_remote: Option<String>,

    /// Recipient name for add-recipient or rm-recipient.
    #[arg(long)]
    pub recipient_name: Option<String>,

    /// Age public key for add-recipient.
    #[arg(long)]
    pub recipient_key: Option<String>,

    /// Target recipient group.
    #[arg(long, default_value = "team")]
    pub group: String,

    /// Workflow file name known to GitHub Actions.
    #[arg(long, default_value = "himitsu.yml")]
    pub workflow: String,

    /// Print the gh command instead of executing it.
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(args: CiArgs) -> Result<()> {
    match args.command {
        CiCommand::Status(args) => status(&args.path),
        CiCommand::Install(args) => install(args),
        CiCommand::Run(args) => trigger(args),
    }
}

fn status(path: &Path) -> Result<()> {
    if path.exists() {
        println!("himitsu ci installed at {}", path.display());
    } else {
        println!(
            "himitsu ci not installed; run `himitsu ci install` to create {}",
            path.display()
        );
    }
    Ok(())
}

fn install(args: InstallArgs) -> Result<()> {
    if let Some(remote) = &args.default_remote {
        config::validate_remote_slug(remote)?;
    }

    if args.path.exists() && !args.force {
        return Err(HimitsuError::External(format!(
            "{} already exists; pass --force to overwrite",
            args.path.display()
        )));
    }

    if let Some(parent) = args.path.parent() {
        fs::create_dir_all(parent)?;
    }

    let workflow = workflow_template(args.default_remote.as_deref(), &args.action_ref);
    fs::write(&args.path, workflow)?;
    println!("Installed himitsu ci workflow at {}", args.path.display());
    Ok(())
}

fn trigger(args: RunArgs) -> Result<()> {
    if let Some(remote) = &args.target_remote {
        config::validate_remote_slug(remote)?;
    }

    let mut fields = vec![format!("operation={}", args.operation.as_input())];
    push_optional_field(&mut fields, "remote", args.target_remote.as_deref());
    push_optional_field(
        &mut fields,
        "recipient-name",
        args.recipient_name.as_deref(),
    );
    push_optional_field(&mut fields, "recipient-key", args.recipient_key.as_deref());
    fields.push(format!("group={}", args.group));

    if args.dry_run {
        println!("{}", render_gh_command(&args.workflow, &fields));
        return Ok(());
    }

    let mut cmd = Command::new("gh");
    cmd.args(["workflow", "run", &args.workflow]);
    for field in &fields {
        cmd.args(["--field", field]);
    }

    let status = cmd.status().map_err(|e| {
        HimitsuError::External(format!(
            "failed to run `gh workflow run {}`: {e}",
            args.workflow
        ))
    })?;

    if !status.success() {
        return Err(HimitsuError::External(format!(
            "gh workflow run exited with status {status}"
        )));
    }

    Ok(())
}

fn push_optional_field(fields: &mut Vec<String>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        if !value.is_empty() {
            fields.push(format!("{name}={value}"));
        }
    }
}

fn render_gh_command(workflow: &str, fields: &[String]) -> String {
    let mut parts = vec![
        "gh".to_string(),
        "workflow".to_string(),
        "run".to_string(),
        workflow.to_string(),
    ];

    for field in fields {
        parts.push("--field".to_string());
        parts.push(field.clone());
    }

    parts.join(" ")
}

fn workflow_template(default_remote: Option<&str>, action_ref: &str) -> String {
    let default_remote = default_remote.unwrap_or("");
    format!(
        r#"name: Himitsu Self-Serve Rekey

on:
  workflow_dispatch:
    inputs:
      operation:
        description: "Operation to run"
        required: true
        default: sync
        type: choice
        options:
          - sync
          - add-recipient
          - rm-recipient
      remote:
        description: "Target himitsu remote (org/repo)"
        required: false
        default: {default_remote}
      recipient-name:
        description: "Recipient name for add-recipient or rm-recipient"
        required: false
      recipient-key:
        description: "Age public key for add-recipient"
        required: false
      group:
        description: "Target recipient group"
        required: false
        default: team

permissions:
  contents: write
  pull-requests: write

jobs:
  himitsu:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: darkmatter/himitsu@{action_ref}
        with:
          operation: ${{{{ inputs.operation }}}}
          remote: ${{{{ inputs.remote }}}}
          recipient-name: ${{{{ inputs.recipient-name }}}}
          recipient-key: ${{{{ inputs.recipient-key }}}}
          group: ${{{{ inputs.group }}}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_inputs_match_action_operations() {
        assert_eq!(CiOperation::Sync.as_input(), "sync");
        assert_eq!(CiOperation::AddRecipient.as_input(), "add-recipient");
        assert_eq!(CiOperation::RmRecipient.as_input(), "rm-recipient");
    }

    #[test]
    fn dry_run_command_includes_fields() {
        let fields = vec![
            "operation=sync".to_string(),
            "remote=acme/secrets".to_string(),
        ];
        assert_eq!(
            render_gh_command("himitsu.yml", &fields),
            "gh workflow run himitsu.yml --field operation=sync --field remote=acme/secrets"
        );
    }
}
