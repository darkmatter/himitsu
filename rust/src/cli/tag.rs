use clap::{Args, Subcommand};

use super::Context;
use crate::crypto::{age, secret_value, tags};
use crate::error::{HimitsuError, Result};
use crate::proto::SecretValue;
use crate::reference::SecretRef;
use crate::remote::store;

/// Manage tags on a secret.
#[derive(Debug, Args)]
pub struct TagArgs {
    /// Secret path. Accepts a bare path (`prod/API_KEY`) or a provider-prefixed
    /// qualified reference (`github:org/repo/prod/API_KEY`) that overrides the
    /// default store.
    pub path: String,

    #[command(subcommand)]
    pub action: TagAction,
}

#[derive(Debug, Subcommand)]
pub enum TagAction {
    /// Add one or more tags. Idempotent — tags already on the secret are not
    /// duplicated.
    Add {
        /// Tag(s) to add. Each must match `[A-Za-z0-9_.-]+`, 1-64 chars.
        #[arg(required = true)]
        tags: Vec<String>,
    },

    /// Remove one or more tags. No-op for tags not currently set.
    Rm {
        /// Tag(s) to remove.
        #[arg(required = true)]
        tags: Vec<String>,
    },

    /// List current tags, one per line, sorted alphabetically.
    List,
}

pub fn run(args: TagArgs, ctx: &Context) -> Result<()> {
    // Validate every input tag *before* any I/O so a malformed input never
    // triggers a decrypt cycle.
    let validated = validate_action(&args.action)?;

    let secret_ref = SecretRef::parse(&args.path)?;
    let (effective_store, secret_path, recipients_path_override) = if secret_ref.is_qualified() {
        let resolved = secret_ref.resolve_store()?;
        let path = secret_ref.path.ok_or_else(|| {
            HimitsuError::InvalidReference(
                "qualified reference must include a secret path after org/repo".into(),
            )
        })?;
        (resolved, path, None)
    } else {
        let path = secret_ref.path.expect("bare SecretRef always has a path");
        (ctx.store.clone(), path, ctx.recipients_path.as_deref())
    };

    let ciphertext = store::read_secret(&effective_store, &secret_path)?;
    let identities = ctx.load_identities()?;
    let plaintext = age::decrypt_with_identities(&ciphertext, &identities)?;
    let mut decoded = secret_value::decode(&plaintext);

    match validated {
        ValidatedAction::List => {
            decoded.tags.sort();
            for t in &decoded.tags {
                println!("{t}");
            }
            return Ok(());
        }
        ValidatedAction::Add(additions) => apply_add(&mut decoded.tags, &additions),
        ValidatedAction::Rm(removals) => apply_rm(&mut decoded.tags, &removals),
    }

    let recipients = age::collect_recipients(&effective_store, recipients_path_override)?;
    if recipients.is_empty() {
        return Err(HimitsuError::Recipient(
            "no recipients found; run `himitsu init` or add recipients first".into(),
        ));
    }

    let sv = SecretValue {
        data: decoded.data,
        content_type: String::new(),
        annotations: decoded.annotations,
        totp: decoded.totp,
        url: decoded.url,
        expires_at: decoded.expires_at,
        description: decoded.description,
        env_key: decoded.env_key,
        tags: decoded.tags,
    };

    let new_plaintext = secret_value::encode(&sv);
    let new_ciphertext = age::encrypt(&new_plaintext, &recipients)?;
    store::write_secret(&effective_store, &secret_path, &new_ciphertext)?;
    println!("Updated tags for {secret_path}");
    Ok(())
}

/// Resolved action with all tag inputs validated.
enum ValidatedAction {
    Add(Vec<String>),
    Rm(Vec<String>),
    List,
}

fn validate_action(action: &TagAction) -> Result<ValidatedAction> {
    match action {
        TagAction::Add { tags: raw } => Ok(ValidatedAction::Add(validate_tag_list(raw)?)),
        TagAction::Rm { tags: raw } => Ok(ValidatedAction::Rm(validate_tag_list(raw)?)),
        TagAction::List => Ok(ValidatedAction::List),
    }
}

fn validate_tag_list(raw: &[String]) -> Result<Vec<String>> {
    raw.iter()
        .map(|t| {
            tags::validate_tag(t)
                .map(|s| s.to_string())
                .map_err(|reason| {
                    HimitsuError::InvalidReference(format!("invalid tag {t:?}: {reason}"))
                })
        })
        .collect()
}

/// Append `additions` to `tags`, skipping any already present. Pre-existing
/// order is preserved; new tags are appended in input order, deduped.
pub(crate) fn apply_add(tags: &mut Vec<String>, additions: &[String]) {
    for t in additions {
        if !tags.iter().any(|existing| existing == t) {
            tags.push(t.clone());
        }
    }
}

/// Remove every entry in `removals` from `tags`. No-op for entries not
/// currently present.
pub(crate) fn apply_rm(tags: &mut Vec<String>, removals: &[String]) {
    tags.retain(|t| !removals.iter().any(|r| r == t));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_appends_new_tag() {
        let mut tags = vec!["pci".to_string()];
        apply_add(&mut tags, &["stripe".to_string()]);
        assert_eq!(tags, vec!["pci".to_string(), "stripe".to_string()]);
    }

    #[test]
    fn add_is_idempotent() {
        let mut tags = vec!["pci".to_string(), "stripe".to_string()];
        apply_add(&mut tags, &["pci".to_string(), "stripe".to_string()]);
        assert_eq!(tags, vec!["pci".to_string(), "stripe".to_string()]);
    }

    #[test]
    fn add_preserves_existing_order() {
        let mut tags = vec!["zeta".to_string(), "alpha".to_string()];
        apply_add(&mut tags, &["mu".to_string()]);
        assert_eq!(
            tags,
            vec!["zeta".to_string(), "alpha".to_string(), "mu".to_string(),]
        );
    }

    #[test]
    fn add_dedups_within_input() {
        let mut tags = vec![];
        apply_add(&mut tags, &["foo".to_string(), "foo".to_string()]);
        assert_eq!(tags, vec!["foo".to_string()]);
    }

    #[test]
    fn rm_removes_listed_tags() {
        let mut tags = vec![
            "pci".to_string(),
            "stripe".to_string(),
            "mobile".to_string(),
        ];
        apply_rm(&mut tags, &["pci".to_string(), "mobile".to_string()]);
        assert_eq!(tags, vec!["stripe".to_string()]);
    }

    #[test]
    fn rm_missing_tag_is_noop() {
        let mut tags = vec!["pci".to_string()];
        apply_rm(&mut tags, &["never-set".to_string()]);
        assert_eq!(tags, vec!["pci".to_string()]);
    }

    #[test]
    fn rm_all_clears_tags() {
        let mut tags = vec!["a".to_string(), "b".to_string()];
        apply_rm(&mut tags, &["a".to_string(), "b".to_string()]);
        assert!(tags.is_empty());
    }

    #[test]
    fn validate_rejects_invalid_input_tag() {
        let action = TagAction::Add {
            tags: vec!["good".to_string(), "bad tag".to_string()],
        };
        match validate_action(&action) {
            Err(HimitsuError::InvalidReference(msg)) => {
                assert!(msg.contains("invalid tag"), "got: {msg}");
                assert!(msg.contains("bad tag"), "got: {msg}");
            }
            Err(other) => panic!("expected InvalidReference, got {other}"),
            Ok(_) => panic!("expected validation error"),
        }
    }

    #[test]
    fn validate_accepts_well_formed_tags() {
        let action = TagAction::Add {
            tags: vec!["pci".to_string(), "rotate-2026-q1".to_string()],
        };
        assert!(validate_action(&action).is_ok());
    }

    #[test]
    fn validate_list_action_is_always_ok() {
        assert!(validate_action(&TagAction::List).is_ok());
    }
}
