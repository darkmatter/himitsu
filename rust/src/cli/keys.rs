use clap::Args;

use super::Context;
use crate::error::{HimitsuError, Result};

/// Print age keys for self or a named recipient.
#[derive(Debug, Args)]
pub struct KeysArgs {
    #[command(subcommand)]
    pub command: Option<KeysCommand>,
}

#[derive(Debug, clap::Subcommand)]
pub enum KeysCommand {
    /// Print public key(s).
    Public {
        /// Recipient name. Use `self` or `me` for own key. Omit for own key.
        ref_: Option<String>,
    },
    /// Print private key (own key only).
    Private {
        /// Must be `self`, `me`, or omitted.
        ref_: Option<String>,
    },
}

fn is_self_ref(r: Option<&str>) -> bool {
    match r {
        None | Some("self") | Some("me") => true,
        _ => false,
    }
}

pub fn run(args: KeysArgs, ctx: &Context) -> Result<()> {
    match args.command {
        // No subcommand → same as `public self`
        None => print_own_public_key(ctx),

        Some(KeysCommand::Public { ref_ }) => {
            if is_self_ref(ref_.as_deref()) {
                print_own_public_key(ctx)
            } else {
                let name = ref_.as_deref().unwrap();
                print_recipient_public_key(ctx, name)
            }
        }

        Some(KeysCommand::Private { ref_ }) => {
            if is_self_ref(ref_.as_deref()) {
                print_own_private_key(ctx)
            } else {
                Err(HimitsuError::Recipient(
                    "private keys are not stored for other recipients".into(),
                ))
            }
        }
    }
}

fn print_own_public_key(ctx: &Context) -> Result<()> {
    let path = crate::crypto::keystore::pubkey_path(&ctx.data_dir);
    let contents = std::fs::read_to_string(&path).map_err(|e| {
        HimitsuError::Io(std::io::Error::new(
            e.kind(),
            format!("could not read public key at {}: {e}", path.display()),
        ))
    })?;
    println!("{}", contents.trim());
    Ok(())
}

fn print_own_private_key(ctx: &Context) -> Result<()> {
    let identity = ctx.load_identity()?;
    let secret_str = secrecy::ExposeSecret::expose_secret(&identity.to_string()).to_string();
    println!("{}", secret_str.trim());
    Ok(())
}

fn print_recipient_public_key(ctx: &Context, name: &str) -> Result<()> {
    // Resolve store: ctx.store may be empty if Keys is not in needs_store.
    let store = if ctx.store.as_os_str().is_empty() {
        let resolved = crate::config::resolve_store(None).unwrap_or_default();
        if resolved.as_os_str().is_empty() {
            return Err(HimitsuError::StoreNotFound(
                "no store resolved — use --store or --remote to specify one".into(),
            ));
        }
        resolved
    } else {
        ctx.store.clone()
    };

    let pub_path =
        crate::remote::store::recipients_dir_with_override(&store, ctx.recipients_path.as_deref())
            .join(format!("{name}.pub"));

    if !pub_path.exists() {
        return Err(HimitsuError::SecretNotFound(format!(
            "recipient '{name}' not found"
        )));
    }

    let contents = std::fs::read_to_string(&pub_path)?;
    println!("{}", contents.trim());
    Ok(())
}
