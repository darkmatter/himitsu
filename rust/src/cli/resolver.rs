//! Secret resolution: reference → store lookup → decrypt → decode.
//!
//! [`SecretResolver`] encapsulates the pipeline that turns a secret reference
//! (bare path like `prod/API_KEY` or qualified ref like
//! `github:org/repo/prod/API_KEY`) into a decrypted
//! [`Decoded`](crate::crypto::secret_value::Decoded) value.

use super::Context;
use crate::crypto::{age, secret_value};
use crate::error::{HimitsuError, Result};
use crate::reference::SecretRef;
use crate::remote::store;

/// Resolves secret references through the full decrypt-decode pipeline.
pub struct SecretResolver;

impl SecretResolver {
    /// Resolve a secret reference to its decrypted, decoded value.
    ///
    /// This is the full pipeline:
    /// 1. Parse the reference (bare path or qualified ref)
    /// 2. Resolve to the effective store path
    /// 3. Read the secret metadata and payload from the store
    /// 4. Decrypt with the user's age identities
    /// 5. Decode the plaintext into a structured `Decoded`
    pub fn resolve(ctx: &Context, path: &str) -> Result<secret_value::Decoded> {
        let identities = ctx.load_identities()?;
        Self::resolve_with_identities(ctx, path, &identities)
    }

    /// Same as [`resolve`](Self::resolve) but reuses pre-loaded identities. Use
    /// this when decrypting many secrets in a loop so key files aren't re-parsed
    /// per iteration (e.g. `himitsu exec` over a glob or env label).
    pub fn resolve_with_identities(
        ctx: &Context,
        path: &str,
        identities: &[::age::x25519::Identity],
    ) -> Result<secret_value::Decoded> {
        let secret_ref = SecretRef::parse(path)?;

        let (effective_store, secret_path) = if secret_ref.is_qualified() {
            let resolved = secret_ref.resolve_store()?;
            let sp = secret_ref.path.ok_or_else(|| {
                HimitsuError::InvalidReference(
                    "qualified reference must include a secret path after org/repo".into(),
                )
            })?;
            (resolved, sp)
        } else {
            let sp = secret_ref.path.expect("bare SecretRef always has a path");
            (ctx.store.clone(), sp)
        };

        let meta = store::read_secret_meta(&effective_store, &secret_path)?;
        let payload = store::read_secret_payload(&effective_store, &secret_path)?;

        match age::decrypt_with_identities(&payload.ciphertext, identities) {
            Ok(plaintext) => Ok(secret_value::decode_with_legacy_environment(
                &plaintext,
                payload.legacy_environment.as_deref(),
            )),
            Err(_) if payload.legacy_proto_envelope => {
                Ok(secret_value::decode_with_legacy_environment(
                    &payload.ciphertext,
                    payload.legacy_environment.as_deref(),
                ))
            }
            Err(_) => {
                let named = super::get::named_recipients(&effective_store, &meta.recipients);
                let loaded: Vec<String> = identities
                    .iter()
                    .map(|id| id.to_public().to_string())
                    .collect();
                let mut msg = String::from("no matching key\n  encrypted for:\n");
                for n in &named {
                    msg.push_str(&format!("    {n}\n"));
                }
                msg.push_str("  loaded identities:\n");
                if loaded.is_empty() {
                    msg.push_str("    (none)\n");
                }
                for id in &loaded {
                    msg.push_str(&format!("    {id}\n"));
                }
                msg.push_str(
                    "  hint: run 'himitsu rekey' if your current identity should have access",
                );
                Err(HimitsuError::DecryptionFailed(msg))
            }
        }
    }
}
