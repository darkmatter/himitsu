//! OutputResolver — the deepened Output pipeline.
//!
//! One module owns the path from the project config `outputs:` map to
//! decoded entries: candidates with decrypted tags → selector/alias
//! resolution → values. Consumed by `exec`, `generate`, and `codegen`
//! (and, later, the TUI outputs view), replacing the pipeline those
//! commands previously assembled by hand — including the
//! `tags: vec![]` bug class where tag selectors silently matched
//! nothing, and the double decryption where candidates were decrypted
//! for tags and entries decrypted again for values.
//!
//! Invariants (see CONTEXT.md):
//! - **Cheap empty**: no project config or no `outputs:` map means no
//!   identity load, no store I/O, no decryption.
//! - **Decrypt once**: `open` performs the single scan; tags for
//!   selector matching and plaintexts for later materialization come
//!   from the same pass. Secrets the current Identity cannot decrypt —
//!   including when no identity loads at all — contribute no tags
//!   (mirroring `ls --tag`) and only error when a value is actually
//!   needed.
//! - **Whole-map validation**: selector parse errors and alias
//!   exactly-one violations surface at `open`, even when the caller
//!   eventually asks for a different Output.
//! - **Tri-state lookup**: [`OutputResolver::env_map`] returns
//!   `Ok(None)` iff the label is not a defined Output, so `exec` can
//!   fall through to selector parsing; real failures are `Err`.
//! - **Channel-free materialization**: [`DecodedEntry`] carries expiry
//!   data; callers decide where warnings render (CLI stderr, TUI badge).

use std::collections::BTreeMap;

use super::Context;
use crate::config::outputs::dsl::OutputsMap;
use crate::config::outputs::resolver::{
    Context as ResolverContext, ResolvedOutput, SecretCandidate, resolve_outputs,
};
use crate::crypto::secret_value;
use crate::error::{HimitsuError, Result};
use crate::remote::store;

/// One decoded env-var binding from an Output.
#[derive(Debug, Clone)]
pub struct DecodedEntry {
    /// The env-var name (alias key, or derived from the secret path).
    pub env_key: String,
    /// The secret path within its store.
    pub secret_path: String,
    /// `Some` = cross-store entry (store slug); `None` = the active store.
    /// Not yet read by the CLI callers — the TUI outputs view renders it.
    #[allow(dead_code)]
    pub store_slug: Option<String>,
    /// UTF-8-validated plaintext value.
    pub value: String,
    /// Expiration timestamp, when set on the Secret.
    pub expires_at: Option<pbjson_types::Timestamp>,
}

impl DecodedEntry {
    /// Canonical expired-secret warning text, when this entry is expired.
    /// Materialization is channel-free: CLI callers print this to stderr,
    /// the TUI renders it as a badge.
    pub fn expiry_warning(&self) -> Option<String> {
        super::get::expiry_message(&self.secret_path, self.expires_at.as_ref())
    }
}

/// The project's `outputs:` map, resolved against the active store.
///
/// [`OutputResolver::open`] is the one expensive call; everything after it
/// reads the snapshot. Borrowing [`Context`] encodes the invariant that a
/// resolver opened against one store is never materialized against another.
pub struct OutputResolver<'ctx> {
    ctx: &'ctx Context,
    /// Post-brace-expansion resolved Outputs, declaration order.
    resolved: Vec<ResolvedOutput>,
    /// Candidates from the `open` scan (path + decrypted tags), reused by
    /// [`Self::resolve_map`] so previews don't re-decrypt the store.
    #[allow(dead_code)]
    candidates: Vec<SecretCandidate>,
    /// Plaintext-decoded local Secrets from the single `open` scan.
    decoded: BTreeMap<String, secret_value::Decoded>,
    identities: Vec<::age::x25519::Identity>,
}

impl<'ctx> OutputResolver<'ctx> {
    /// Resolve the invocation's `outputs:` map (via
    /// [`Context::project_config`]) against the active store.
    ///
    /// Cheap when no Outputs are defined: no identity load, no store I/O,
    /// no decryption. Otherwise this is the single decrypt-once scan.
    ///
    /// The scan is tolerant: missing identities or an unreadable store
    /// yield candidates without tags (they simply match no `tag:`
    /// selectors, mirroring `ls --tag`) rather than failing resolution —
    /// `codegen --lang` legitimately runs without any identity on disk.
    /// Needing a *value* still fails hard, at [`Self::env_map`]/
    /// [`Self::decode`] time.
    pub fn open(ctx: &'ctx Context) -> Result<Self> {
        let outputs_map = ctx
            .project_config()?
            .map(|(cfg, _)| cfg.codegen)
            .unwrap_or_default();
        if outputs_map.is_empty() {
            return Ok(Self {
                ctx,
                resolved: Vec::new(),
                candidates: Vec::new(),
                decoded: BTreeMap::new(),
                identities: Vec::new(),
            });
        }

        let identities = ctx.load_identities().unwrap_or_default();
        let all_paths = store::list_secrets(&ctx.store, None).unwrap_or_default();
        let mut decoded: BTreeMap<String, secret_value::Decoded> = BTreeMap::new();
        let mut candidates = Vec::with_capacity(all_paths.len());
        for path in all_paths {
            let tags = match super::resolver::SecretResolver::resolve_with_identities(
                ctx,
                &path,
                &identities,
            ) {
                Ok(d) => {
                    let tags = d.tags.clone();
                    decoded.insert(path.clone(), d);
                    tags
                }
                Err(_) => Vec::new(),
            };
            candidates.push(SecretCandidate { path, tags });
        }

        let resolver_ctx = ResolverContext {
            available_secrets: candidates.clone(),
        };
        let resolved = resolve_outputs(&outputs_map, &resolver_ctx)?;
        Ok(Self {
            ctx,
            resolved,
            candidates,
            decoded,
            identities,
        })
    }

    /// All resolved Outputs (post brace-expansion), declaration order.
    /// Metadata only — no plaintext values.
    pub fn all(&self) -> &[ResolvedOutput] {
        &self.resolved
    }

    /// One resolved Output by exact (post-expansion) name.
    pub fn get(&self, name: &str) -> Option<&ResolvedOutput> {
        self.resolved.iter().find(|o| o.name == name)
    }

    /// The strict `exec` surface: one label → injection-ready env map.
    ///
    /// Tri-state: `Ok(None)` iff `label` is not a defined Output (the
    /// caller falls through to selector parsing); `Ok(Some(map))` when
    /// defined — the map may be empty (emptiness policy is the caller's);
    /// everything else is `Err`.
    ///
    /// Local-store only: any cross-store entry is a hard
    /// [`HimitsuError::NotSupported`]. `tag_filter` is AND-applied to each
    /// entry's decrypted tags *before* env-key conflict detection, so a
    /// filtered-out entry can never cause a conflict.
    pub fn env_map(
        &self,
        label: &str,
        tag_filter: &[String],
    ) -> Result<Option<BTreeMap<String, String>>> {
        let Some(output) = self.get(label) else {
            return Ok(None);
        };

        let mut env_map: BTreeMap<String, (String, String)> = BTreeMap::new();
        for entry in &output.entries {
            if entry.store_slug.is_some() {
                return Err(HimitsuError::NotSupported(format!(
                    "output {label:?} references a cross-store secret ({}); \
                     cross-store exec is not supported yet",
                    entry.secret_path
                )));
            }

            let decoded = self.decoded_for(entry)?;

            // Apply the `--tag` AND filter on top of the resolved selection.
            if !tag_filter.is_empty()
                && !tag_filter
                    .iter()
                    .all(|t| decoded.tags.iter().any(|d| d == t))
            {
                continue;
            }

            let key = entry.env_key.clone();
            super::set::validate_env_key(&key).map_err(|e| {
                HimitsuError::InvalidReference(format!("{e} (from {:?})", entry.secret_path))
            })?;
            if let Some((_, prev_path)) = env_map.get(&key) {
                return Err(HimitsuError::InvalidConfig(format!(
                    "env-var {key:?} would be set by both {prev_path:?} and {:?}; \
                     rename one via `set --env-key` or a selector alias",
                    entry.secret_path
                )));
            }
            let value = String::from_utf8(decoded.data).map_err(|e| {
                HimitsuError::InvalidReference(format!(
                    "secret {:?} contains non-UTF-8 bytes — exec can only inject text values: {e}",
                    entry.secret_path
                ))
            })?;
            env_map.insert(key, (value, entry.secret_path.clone()));
        }

        Ok(Some(
            env_map.into_iter().map(|(k, (v, _))| (k, v)).collect(),
        ))
    }

    /// Decode an Output's entries to plaintext values.
    ///
    /// Follows cross-store entries (via [`crate::config::ensure_store`]).
    /// Preserves entry order and duplicate env keys — callers own their
    /// collapse policy (generate warns then last-wins; codegen sops mode
    /// silently last-wins; exec's strict collapse is [`Self::env_map`]).
    pub fn decode(&self, output: &ResolvedOutput) -> Result<Vec<DecodedEntry>> {
        let mut entries = Vec::with_capacity(output.entries.len());
        for entry in &output.entries {
            let decoded = if let Some(ref slug) = entry.store_slug {
                let effective_store = crate::config::ensure_store(slug)?;
                let payload = store::read_secret_payload(&effective_store, &entry.secret_path)?;
                match crate::crypto::age::decrypt_with_identities(
                    &payload.ciphertext,
                    &self.identities,
                ) {
                    Ok(p) => secret_value::decode_with_legacy_environment(
                        &p,
                        payload.legacy_environment.as_deref(),
                    ),
                    Err(_) if payload.legacy_proto_envelope => {
                        secret_value::decode_with_legacy_environment(
                            &payload.ciphertext,
                            payload.legacy_environment.as_deref(),
                        )
                    }
                    Err(err) => return Err(err),
                }
            } else {
                self.decoded_for(entry)?
            };

            let value = String::from_utf8(decoded.data).map_err(|e| {
                HimitsuError::DecryptionFailed(format!(
                    "non-UTF-8 secret at '{}': {e}",
                    entry.secret_path
                ))
            })?;
            entries.push(DecodedEntry {
                env_key: entry.env_key.clone(),
                secret_path: entry.secret_path.clone(),
                store_slug: entry.store_slug.clone(),
                value,
                expires_at: decoded.expires_at,
            });
        }
        Ok(entries)
    }

    /// Resolve an arbitrary `outputs:` map against the candidates built at
    /// [`Self::open`] — the seam for previewing an edited-but-unsaved map
    /// (TUI DSL editor) and for tests, without re-decrypting the store.
    #[allow(dead_code)]
    pub fn resolve_map(&self, outputs: &OutputsMap) -> Result<Vec<ResolvedOutput>> {
        let resolver_ctx = ResolverContext {
            available_secrets: self.candidates.clone(),
        };
        resolve_outputs(outputs, &resolver_ctx)
    }

    /// A local entry's decoded value: from the `open` scan's cache when
    /// available (the common case), otherwise decrypted now with the rich
    /// [`SecretResolver`](super::resolver::SecretResolver) diagnostic — this
    /// is where an undecryptable Secret that selectors tolerated becomes a
    /// hard error because its *value* is needed.
    fn decoded_for(
        &self,
        entry: &crate::config::outputs::resolver::ResolvedEntry,
    ) -> Result<secret_value::Decoded> {
        match self.decoded.get(&entry.secret_path) {
            Some(d) => Ok(d.clone()),
            None => super::resolver::SecretResolver::resolve_with_identities(
                self.ctx,
                &entry.secret_path,
                &self.identities,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use secrecy::ExposeSecret;

    use super::*;
    use crate::crypto::age as hage;
    use crate::proto::SecretValue;

    /// A Context over a tempdir store with one real age identity on disk,
    /// recipients configured, and `project_root` pointing at an (initially
    /// config-less) project directory.
    fn test_ctx() -> (tempfile::TempDir, Context) {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let state_dir = tmp.path().join("state");
        let store = tmp.path().join("store");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/recipients")).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/secrets")).unwrap();
        std::fs::create_dir_all(&project).unwrap();

        let identity = ::age::x25519::Identity::generate();
        let pubkey = identity.to_public().to_string();
        std::fs::write(data_dir.join("key"), identity.to_string().expose_secret()).unwrap();
        std::fs::write(
            store.join(".himitsu/recipients/me.pub"),
            format!("{pubkey}\n"),
        )
        .unwrap();

        let ctx = Context {
            data_dir,
            state_dir,
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
            project_root: Some(project),
            git: Arc::new(crate::git::CliGitAdapter),
            project_config_cell: Default::default(),
        };
        (tmp, ctx)
    }

    fn write_project_outputs(ctx: &Context, yaml: &str) {
        let root = ctx.project_root.as_ref().unwrap();
        std::fs::write(root.join("himitsu.yaml"), yaml).unwrap();
    }

    fn write_secret_with(
        store_path: &Path,
        path: &str,
        data: &[u8],
        tags: &[&str],
        recipients: &[::age::x25519::Recipient],
    ) {
        let sv = SecretValue {
            data: data.to_vec(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let wire = secret_value::encode(&sv);
        let ct = hage::encrypt(&wire, recipients).unwrap();
        store::write_secret(store_path, path, &ct).unwrap();
    }

    fn store_recipients(ctx: &Context) -> Vec<::age::x25519::Recipient> {
        hage::collect_recipients(&ctx.store, None).unwrap()
    }

    #[test]
    fn open_is_cheap_when_no_outputs_are_defined() {
        let (_tmp, mut ctx) = test_ctx();
        // No project config at all, no identity on disk, and a store path
        // that doesn't exist: `open` must still succeed because the empty
        // outputs map short-circuits before any identity load or store I/O.
        ctx.data_dir = ctx.data_dir.join("nonexistent");
        ctx.store = ctx.store.join("nonexistent");

        let outputs = OutputResolver::open(&ctx).expect("cheap empty open");
        assert!(outputs.all().is_empty());
        assert_eq!(outputs.env_map("anything", &[]).unwrap(), None);
        assert!(outputs.get("anything").is_none());
    }

    #[test]
    fn open_tolerates_missing_identities() {
        // CI regression: `codegen --lang` legitimately runs on machines with
        // no age identity at all. The scan must tolerate identity-load
        // failure (candidates carry no tags, tag selectors match nothing) —
        // only needing a VALUE fails hard.
        let (_tmp, ctx) = test_ctx();
        let recipients = store_recipients(&ctx);
        write_secret_with(&ctx.store, "prod/api-key", b"v", &["pci"], &recipients);
        std::fs::remove_file(ctx.data_dir.join("key")).unwrap();
        write_project_outputs(
            &ctx,
            "codegen:\n  app:\n    selectors:\n      - tag:pci\n  direct:\n    selectors:\n      - prod/api-key\n",
        );

        let outputs = OutputResolver::open(&ctx).expect("open tolerates no identities");
        // Tags are unreadable, so the tag selector matches nothing.
        let env = outputs.env_map("app", &[]).unwrap().expect("defined");
        assert!(env.is_empty(), "{env:?}");
        // A concrete-path entry needs the value: hard error.
        assert!(outputs.env_map("direct", &[]).is_err());
    }

    #[test]
    fn env_map_is_tri_state() {
        let (_tmp, ctx) = test_ctx();
        let recipients = store_recipients(&ctx);
        write_secret_with(&ctx.store, "prod/api-key", b"v1", &["pci"], &recipients);
        write_project_outputs(&ctx, "codegen:\n  app:\n    selectors:\n      - tag:pci\n");

        let outputs = OutputResolver::open(&ctx).unwrap();
        // Unknown label: not an error — the caller falls through to
        // selector parsing.
        assert_eq!(outputs.env_map("nope", &[]).unwrap(), None);
        // Defined label: resolved with real decrypted tags, value from the
        // same single scan.
        let env = outputs.env_map("app", &[]).unwrap().expect("defined");
        assert_eq!(env.get("API_KEY").map(String::as_str), Some("v1"));
    }

    #[test]
    fn undecryptable_secret_matches_no_tags_but_value_is_hard_error() {
        let (_tmp, ctx) = test_ctx();
        let recipients = store_recipients(&ctx);
        write_secret_with(&ctx.store, "prod/mine", b"ok", &["pci"], &recipients);
        // A secret encrypted to a foreign identity: contributes no tags
        // (tolerated), but erroring when its value is needed.
        let foreign = ::age::x25519::Identity::generate();
        write_secret_with(
            &ctx.store,
            "prod/foreign",
            b"locked",
            &["pci"],
            &[foreign.to_public()],
        );
        write_project_outputs(
            &ctx,
            "codegen:\n  tagged:\n    selectors:\n      - tag:pci\n  direct:\n    selectors: []\n    aliases:\n      LOCKED: prod/foreign\n",
        );

        let outputs = OutputResolver::open(&ctx).unwrap();
        // The foreign secret's tags are unreadable, so tag:pci matches only
        // the decryptable one.
        let env = outputs.env_map("tagged", &[]).unwrap().expect("defined");
        assert_eq!(env.len(), 1);
        assert!(env.contains_key("MINE"));
        // Naming it directly needs the value: hard error.
        let err = outputs.env_map("direct", &[]).unwrap_err();
        assert!(matches!(err, HimitsuError::DecryptionFailed(_)), "{err:?}");
    }

    #[test]
    fn cross_store_entry_is_rejected_by_env_map_before_any_value_work() {
        let (_tmp, ctx) = test_ctx();
        let recipients = store_recipients(&ctx);
        write_secret_with(&ctx.store, "prod/local", b"v", &[], &recipients);
        write_project_outputs(
            &ctx,
            "codegen:\n  app:\n    selectors: []\n    aliases:\n      SHARED: github:acme/other#prod/key\n",
        );

        let outputs = OutputResolver::open(&ctx).unwrap();
        let err = outputs.env_map("app", &[]).unwrap_err();
        match err {
            HimitsuError::NotSupported(msg) => {
                assert!(msg.contains("cross-store"), "{msg}");
                assert!(msg.contains("prod/key"), "{msg}");
            }
            other => panic!("expected NotSupported, got {other:?}"),
        }
        // The metadata surface still shows the entry, slug intact, for
        // observers (decode/preview callers own their policy).
        let entry = &outputs.get("app").unwrap().entries[0];
        assert_eq!(entry.store_slug.as_deref(), Some("acme/other"));
    }

    #[test]
    fn tag_filter_applies_before_conflict_detection() {
        let (_tmp, ctx) = test_ctx();
        let recipients = store_recipients(&ctx);
        // Both paths derive the same env key (API_KEY).
        write_secret_with(&ctx.store, "prod/api-key", b"p", &["prod"], &recipients);
        write_secret_with(&ctx.store, "staging/api-key", b"s", &[], &recipients);
        write_project_outputs(
            &ctx,
            "codegen:\n  app:\n    selectors:\n      - prod/*\n      - staging/*\n",
        );

        let outputs = OutputResolver::open(&ctx).unwrap();
        // Unfiltered: two survivors bind API_KEY — hard conflict naming both.
        let err = outputs.env_map("app", &[]).unwrap_err();
        match err {
            HimitsuError::InvalidConfig(msg) => {
                assert!(msg.contains("prod/api-key"), "{msg}");
                assert!(msg.contains("staging/api-key"), "{msg}");
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
        // Filtered: the untagged collider is dropped before the conflict
        // check, so this succeeds with one binding.
        let env = outputs
            .env_map("app", &["prod".to_string()])
            .unwrap()
            .expect("defined");
        assert_eq!(env.get("API_KEY").map(String::as_str), Some("p"));
    }

    #[test]
    fn alias_exactly_one_violation_surfaces_at_open() {
        let (_tmp, ctx) = test_ctx();
        let recipients = store_recipients(&ctx);
        write_secret_with(&ctx.store, "prod/x", b"v", &[], &recipients);
        // Selector-valued alias matching zero secrets: whole-map validation
        // fails at open, even though a caller may want a different output.
        write_project_outputs(
            &ctx,
            "codegen:\n  ok:\n    selectors:\n      - prod/x\n  broken:\n    selectors: []\n    aliases:\n      K: tag:nomatch\n",
        );

        assert!(OutputResolver::open(&ctx).is_err());
    }

    #[test]
    fn decode_preserves_duplicates_order_and_metadata() {
        let (_tmp, ctx) = test_ctx();
        let recipients = store_recipients(&ctx);
        write_secret_with(&ctx.store, "prod/api-key", b"p", &[], &recipients);
        write_secret_with(&ctx.store, "staging/api-key", b"s", &[], &recipients);
        write_project_outputs(
            &ctx,
            "codegen:\n  app:\n    selectors:\n      - prod/*\n      - staging/*\n",
        );

        let outputs = OutputResolver::open(&ctx).unwrap();
        let resolved = outputs.get("app").unwrap();
        let entries = outputs.decode(resolved).unwrap();
        // Duplicate env keys preserved in entry order — collapse policy
        // belongs to the caller (generate warns, codegen last-wins).
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.env_key == "API_KEY"));
        assert!(entries.iter().all(|e| e.store_slug.is_none()));
        assert!(entries.iter().all(|e| e.expiry_warning().is_none()));
        let values: Vec<&str> = entries.iter().map(|e| e.value.as_str()).collect();
        assert_eq!(values, ["p", "s"]);
    }

    #[test]
    fn non_utf8_value_is_a_hard_error() {
        let (_tmp, ctx) = test_ctx();
        let recipients = store_recipients(&ctx);
        write_secret_with(
            &ctx.store,
            "prod/blob",
            &[0xff, 0xfe, 0x00],
            &[],
            &recipients,
        );
        write_project_outputs(
            &ctx,
            "codegen:\n  app:\n    selectors:\n      - prod/blob\n",
        );

        let outputs = OutputResolver::open(&ctx).unwrap();
        assert!(matches!(
            outputs.env_map("app", &[]).unwrap_err(),
            HimitsuError::InvalidReference(_)
        ));
        let resolved = outputs.get("app").unwrap();
        assert!(matches!(
            outputs.decode(resolved).unwrap_err(),
            HimitsuError::DecryptionFailed(_)
        ));
    }

    #[test]
    fn resolve_map_previews_an_unsaved_map_against_the_open_scan() {
        let (_tmp, ctx) = test_ctx();
        let recipients = store_recipients(&ctx);
        write_secret_with(&ctx.store, "prod/api-key", b"v", &["pci"], &recipients);
        write_project_outputs(
            &ctx,
            "codegen:\n  app:\n    selectors:\n      - prod/api-key\n",
        );

        let outputs = OutputResolver::open(&ctx).unwrap();
        // Preview a map that is NOT the saved project config (e.g. the TUI
        // DSL editor's buffer) without re-decrypting the store.
        let preview_yaml = "preview:\n  selectors:\n    - tag:pci\n";
        let preview_map: OutputsMap = serde_yaml::from_str(preview_yaml).unwrap();
        let resolved = outputs.resolve_map(&preview_map).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "preview");
        assert_eq!(resolved[0].entries[0].secret_path, "prod/api-key");
    }
}
