use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;

use clap::Args;
use owo_colors::OwoColorize;

use super::Context;
use crate::config::KeyProvider;
use crate::crypto::keystore;
use crate::error::Result;
use crate::remote::store as rstore;

#[derive(Debug, Args)]
pub struct DoctorArgs {}

pub fn run(_args: DoctorArgs, ctx: &Context) -> Result<()> {
    let use_color = std::io::stdout().is_terminal();
    let mut warnings = 0usize;

    println!("{}", "─".repeat(50));

    // ── IDENTITIES ──────────────────────────────────────────────────────────
    println!("IDENTITIES");
    let rdir = rstore::recipients_dir_with_override(&ctx.store, ctx.recipients_path.as_deref());
    let recipient_map = collect_recipient_map(&rdir); // name → pubkey

    let loaded_pubkeys: Vec<String> = ctx
        .load_identities()
        .unwrap_or_default()
        .iter()
        .map(|id| id.to_public().to_string())
        .collect();

    if recipient_map.is_empty() {
        println!("  (no recipients found)");
    } else {
        for (name, pubkey) in &recipient_map {
            let short = short_key(pubkey);
            let can_decrypt = loaded_pubkeys.iter().any(|lp| lp == pubkey.trim());
            let source = if can_decrypt {
                match &ctx.key_provider {
                    KeyProvider::MacosKeychain => "[keychain]",
                    KeyProvider::Disk => "[disk]",
                }
            } else {
                ""
            };
            if can_decrypt {
                let line = format!("  \u{2713} {name:<20} {short}  {source}");
                println!("{}", if use_color { line.green().to_string() } else { line });
            } else {
                warnings += 1;
                let line = format!("  \u{2717} {name:<20} {short}  [no key found]");
                println!("{}", if use_color { line.red().to_string() } else { line });
            }
        }
    }

    // ── SECRETS ─────────────────────────────────────────────────────────────
    println!("\nSECRETS");
    let own_pubkey = std::fs::read_to_string(keystore::pubkey_path(&ctx.data_dir))
        .ok()
        .map(|s| s.trim().to_string());

    // Build pubkey → name reverse map.
    let pubkey_to_name: HashMap<String, String> = recipient_map
        .iter()
        .map(|(n, k)| (k.trim().to_string(), n.clone()))
        .collect();

    let secrets_dir = rstore::secrets_dir(&ctx.store);
    if !secrets_dir.exists() {
        println!("  (no secrets)");
    } else {
        let mut secret_paths = vec![];
        collect_secret_paths(&secrets_dir, &secrets_dir, &mut secret_paths);
        secret_paths.sort();

        if secret_paths.is_empty() {
            println!("  (no secrets)");
        }

        for (rel, abs) in &secret_paths {
            let Ok(contents) = std::fs::read_to_string(abs) else {
                continue;
            };
            let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(&contents) else {
                continue;
            };

            let file_pubkeys: Vec<String> = val["himitsu"]["age"]
                .as_sequence()
                .map(|seq| {
                    seq.iter()
                        .filter_map(|e| e["recipient"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            // Stale: own key not in file's recipient list.
            let self_included = own_pubkey
                .as_ref()
                .map(|own| file_pubkeys.iter().any(|fp| fp.trim() == own.trim()))
                .unwrap_or(true);

            // Orphan: file has a pubkey that no longer has a .pub in recipients/.
            let orphan_keys: Vec<String> = file_pubkeys
                .iter()
                .filter(|fp| !pubkey_to_name.contains_key(fp.trim()))
                .map(|fp| short_key(fp))
                .collect();

            let named: Vec<String> = file_pubkeys
                .iter()
                .map(|pk| {
                    pubkey_to_name
                        .get(pk.trim())
                        .cloned()
                        .unwrap_or_else(|| short_key(pk))
                })
                .collect();

            if !self_included || !orphan_keys.is_empty() {
                warnings += 1;
                if !self_included {
                    let line = format!(
                        "  \u{26a0} {rel:<30}  stale — your identity is not a recipient (recipients: {})",
                        named.join(", ")
                    );
                    println!("{}", if use_color { line.yellow().to_string() } else { line });
                }
                if !orphan_keys.is_empty() {
                    let line = format!(
                        "  \u{26a0} {rel:<30}  orphan recipients: {}",
                        orphan_keys.join(", ")
                    );
                    println!("{}", if use_color { line.yellow().to_string() } else { line });
                }
            } else {
                let line = format!(
                    "  \u{2713} {rel:<30}  encrypted for: {}",
                    named.join(", ")
                );
                println!("{}", if use_color { line.green().to_string() } else { line });
            }
        }
    }

    // ── KEY PROVIDER ─────────────────────────────────────────────────────────
    println!("\nKEY PROVIDER");
    let provider_name = match &ctx.key_provider {
        KeyProvider::MacosKeychain => "macos-keychain",
        KeyProvider::Disk => "disk",
    };
    println!("  \u{2713} provider: {provider_name}");

    let disk_key_path = keystore::disk_secret_path(&ctx.data_dir);
    if matches!(ctx.key_provider, KeyProvider::MacosKeychain) && disk_key_path.exists() {
        warnings += 1;
        println!(
            "  \u{26a0} disk key file still present at {} — consider removing after confirming keychain entry works",
            disk_key_path.display()
        );
    }

    println!("{}", "─".repeat(50));
    if warnings == 0 {
        println!("No issues found.");
    } else {
        println!("{warnings} warning(s)");
    }
    Ok(())
}

fn collect_recipient_map(rdir: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if rdir.exists() {
        walk_recipients(rdir, rdir, &mut map);
    }
    map
}

fn walk_recipients(base: &Path, dir: &Path, map: &mut HashMap<String, String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_recipients(base, &path, map);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("pub") {
            continue;
        }
        let Ok(key) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .with_extension("")
            .to_string_lossy()
            .to_string();
        map.insert(rel, key.trim().to_string());
    }
}

fn collect_secret_paths(
    base: &Path,
    dir: &Path,
    out: &mut Vec<(String, std::path::PathBuf)>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_secret_paths(base, &path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .with_extension("")
            .to_string_lossy()
            .to_string();
        out.push((rel, path));
    }
}

fn short_key(pk: &str) -> String {
    let s = pk.trim();
    if s.len() <= 16 {
        s.to_string()
    } else {
        format!("{}\u{2026}", &s[..12])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_ctx(tmp: &TempDir) -> Context {
        let data_dir = tmp.path().join("data");
        let state_dir = tmp.path().join("state");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/secrets")).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/recipients")).unwrap();
        Context {
            data_dir,
            state_dir,
            store,
            recipients_path: None,
            key_provider: KeyProvider::Disk,
        }
    }

    #[test]
    fn doctor_no_issues_empty_store() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_ctx(&tmp);
        // Should return Ok even with no keys or secrets.
        let result = run(DoctorArgs {}, &ctx);
        assert!(result.is_ok(), "doctor should succeed on empty store");
    }

    #[test]
    fn doctor_stale_secret_runs_ok() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_ctx(&tmp);

        // Write own pubkey (different from the one in the secret).
        let own_pub = "age1ownkey000000000000000000000000000000000000000000000000000";
        std::fs::write(keystore::pubkey_path(&ctx.data_dir), format!("{own_pub}\n")).unwrap();

        // Write a recipient .pub for "alice" with a different key.
        let alice_pub = "age1alice0000000000000000000000000000000000000000000000000000";
        std::fs::write(
            ctx.store.join(".himitsu/recipients/alice.pub"),
            format!("{alice_pub}\n"),
        )
        .unwrap();

        // Write a secret YAML envelope encrypted only for alice (not own key).
        let secret_yaml = format!(
            "value: 'ENC[age,AAAA]'\nhimitsu:\n  created_at: '2026-01-01'\n  lastmodified: '2026-01-01T00:00:00Z'\n  age:\n    - recipient: {alice_pub}\n"
        );
        std::fs::create_dir_all(ctx.store.join(".himitsu/secrets/prod")).unwrap();
        std::fs::write(
            ctx.store.join(".himitsu/secrets/prod/API_KEY.yaml"),
            secret_yaml,
        )
        .unwrap();

        // Doctor should run without panicking and return Ok.
        // (The stale warning is printed but not asserted on here.)
        let result = run(DoctorArgs {}, &ctx);
        assert!(result.is_ok(), "doctor should succeed even with stale secret");
    }
}
