use assert_cmd::Command;
use predicates::prelude::*;
use prost::Message;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[path = "../../rust/src/proto/mod.rs"]
#[allow(unused_imports, dead_code)]
mod proto;

#[allow(deprecated)]
fn himitsu() -> Command {
    Command::cargo_bin("himitsu").unwrap()
}

/// Set up a user home (keys/config) and a project store root.
///
/// In the new model:
/// - HIMITSU_CONFIG → config_dir = parent of the file, data_dir = <parent>/share, state_dir = <parent>/state
/// - `--store <path>` points to the store ROOT (not `.himitsu/` inside it)
/// - Secrets are stored at `store_root/.himitsu/secrets/<path>.yaml`
/// - Recipients at `store_root/.himitsu/recipients/`
fn setup() -> (TempDir, TempDir) {
    let home = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store.path().to_string_lossy(), "init"])
        .assert()
        .success();

    (home, store)
}

#[allow(deprecated)]
fn setup_with_legacy_env_field(
    env_value: &str,
    existing_tag: Option<&str>,
) -> (TempDir, TempDir, String) {
    let (home, store) = setup();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "test/legacy-key",
            "legacy-value",
        ])
        .assert()
        .success();

    let secret_path = store.path().join(".himitsu/secrets/test/legacy-key.age");
    let yaml_path = store.path().join(".himitsu/secrets/test/legacy-key.yaml");
    let _ = std::fs::remove_file(yaml_path);
    let envelope = proto::SecretEnvelope {
        version: 1,
        key_name: "legacy-key".to_string(),
        environment: env_value.to_string(),
        ciphertext: proto::SecretValue {
            data: b"legacy-value".to_vec(),
            tags: existing_tag
                .map(|tag| vec![tag.to_string()])
                .unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec(),
        ..Default::default()
    };
    std::fs::write(&secret_path, envelope.encode_to_vec()).unwrap();

    (home, store, "test/legacy-key".to_string())
}

#[allow(deprecated)]
fn write_legacy_env_secret(
    home: &TempDir,
    store: &TempDir,
    path: &str,
    value: &str,
    env_value: &str,
    existing_tags: &[&str],
) {
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(store), "set", path, value])
        .assert()
        .success();

    let secret_path = store.path().join(format!(".himitsu/secrets/{path}.age"));
    let yaml_path = store.path().join(format!(".himitsu/secrets/{path}.yaml"));
    let _ = std::fs::remove_file(yaml_path);
    if let Some(parent) = secret_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let envelope = proto::SecretEnvelope {
        version: 1,
        key_name: path.rsplit('/').next().unwrap_or(path).to_string(),
        environment: env_value.to_string(),
        ciphertext: proto::SecretValue {
            data: value.as_bytes().to_vec(),
            tags: existing_tags.iter().map(|tag| (*tag).to_string()).collect(),
            ..Default::default()
        }
        .encode_to_vec(),
        ..Default::default()
    };
    std::fs::write(secret_path, envelope.encode_to_vec()).unwrap();
}

fn write_legacy_env_project_config(store: &TempDir) -> PathBuf {
    let config_path = store.path().join(".himitsu.yaml");
    std::fs::write(
        &config_path,
        r#"default_store: local/test
envs:
  app-prod:
    - common/*
    - tag:pci
    - tag:prod
    - STRIPE: tag:stripe
  app-staging:
    - staging/*
generate:
  target: gen
"#,
    )
    .unwrap();
    config_path
}

#[allow(deprecated)]
fn read_legacy_proto_envelope(path: &Path) -> proto::SecretEnvelope {
    proto::SecretEnvelope::decode(std::fs::read(path).unwrap().as_slice()).unwrap()
}

fn collect_file_hashes(root: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                let hash = Sha256::digest(&bytes);
                out.insert(
                    path.strip_prefix(base).unwrap().to_path_buf(),
                    format!("{hash:x}"),
                );
            }
        }
    }

    let mut hashes = BTreeMap::new();
    walk(root, root, &mut hashes);
    hashes
}

#[test]
#[allow(deprecated)]
fn migrate_envs_full_roundtrip() {
    let (home, store) = setup();
    write_legacy_env_secret(&home, &store, "prod/api-key", "prod-secret", "prod", &[]);
    write_legacy_env_secret(
        &home,
        &store,
        "prod/stripe",
        "stripe-secret",
        "prod",
        &["stripe"],
    );
    write_legacy_env_secret(
        &home,
        &store,
        "staging/api-key",
        "staging-secret",
        "staging",
        &[],
    );
    let config_path = write_legacy_env_project_config(&store);
    std::fs::write(home.path().join("state/envs.db"), b"legacy cache").unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "migrate", "envs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("secrets migrated: 3"))
        .stdout(predicate::str::contains("output blocks rewritten: 2"))
        .stdout(predicate::str::contains("cache deleted: true"))
        .stdout(predicate::str::contains("dry-run: false"));

    assert!(store.path().join(".himitsu.yaml.bak").exists());
    assert!(!home.path().join("state/envs.db").exists());
    let migrated_config = std::fs::read_to_string(config_path).unwrap();
    assert!(migrated_config.contains("codegen:"));
    assert!(!migrated_config.contains("envs:"));
    assert!(migrated_config.contains("selectors:"));
    assert!(migrated_config.contains("aliases:"));
    assert!(migrated_config.contains("tag:pci,tag:prod"));

    for (path, tag) in [
        ("prod/api-key", "prod"),
        ("prod/stripe", "prod"),
        ("staging/api-key", "staging"),
    ] {
        let envelope = read_legacy_proto_envelope(&legacy_envelope_path(&store, path));
        assert_eq!(envelope.environment, "");
        himitsu()
            .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
            .args(["--store", &store_flag(&store), "get", path])
            .assert()
            .success()
            .stderr(predicate::str::contains(tag));
    }
}

#[test]
fn migrate_envs_dry_run_is_nondestructive() {
    let (home, store) = setup();
    write_legacy_env_secret(&home, &store, "prod/api-key", "prod-secret", "prod", &[]);
    write_legacy_env_secret(
        &home,
        &store,
        "staging/api-key",
        "staging-secret",
        "staging",
        &[],
    );
    write_legacy_env_project_config(&store);
    std::fs::write(home.path().join("state/envs.db"), b"legacy cache").unwrap();
    let store_before = collect_file_hashes(store.path());
    let home_before = collect_file_hashes(home.path());

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &store_flag(&store),
            "migrate",
            "envs",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("secrets migrated: 2"))
        .stdout(predicate::str::contains("output blocks rewritten: 2"))
        .stdout(predicate::str::contains("cache deleted: true"))
        .stdout(predicate::str::contains("dry-run: true"));

    assert_eq!(store_before, collect_file_hashes(store.path()));
    assert_eq!(home_before, collect_file_hashes(home.path()));
    assert!(!store.path().join(".himitsu.yaml.bak").exists());
}

#[test]
fn migrate_envs_is_idempotent() {
    let (home, store) = setup();
    write_legacy_env_secret(&home, &store, "prod/api-key", "prod-secret", "prod", &[]);
    write_legacy_env_project_config(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "migrate", "envs"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "migrate", "envs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("secrets migrated: 0"))
        .stdout(predicate::str::contains("output blocks rewritten: 0"))
        .stdout(predicate::str::contains("cache deleted: false"));
}

#[test]
fn migrate_envs_rewrites_project_config() {
    let (home, store) = setup();

    // Write a project config carrying a legacy `envs:` block. Before the fix,
    // config deserialization hard-rejected this key, so `migrate envs` could
    // never run (chicken-and-egg). Now it deserializes with a warning.
    let config_path = store.path().join(".himitsu.yaml");
    std::fs::write(
        &config_path,
        "envs:\n  dev:\n    - dev/*\n    - DB_PASSWORD\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "migrate", "envs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("output blocks rewritten: 1"));

    let migrated = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !migrated.contains("envs:"),
        "envs: should be gone: {migrated}"
    );
    assert!(
        migrated.contains("codegen:"),
        "codegen: missing: {migrated}"
    );
    assert!(migrated.contains("dev:"), "dev env missing: {migrated}");
    assert!(
        migrated.contains("selectors:"),
        "selectors missing: {migrated}"
    );
    assert!(
        migrated.contains("dev/*"),
        "selector dev/* missing: {migrated}"
    );
    assert!(
        migrated.contains("DB_PASSWORD"),
        "selector DB_PASSWORD missing: {migrated}"
    );
}

#[test]
fn migrate_envs_rewrites_project_config_in_project_mode() {
    // Regression test for the bug where `migrate envs` did nothing when run
    // inside a project: the migrator only looked at `<store>/.himitsu.yaml`,
    // but in project mode the legacy `envs:` block lives in the repo's own
    // project config (himitsu.yaml), discovered by walking up from cwd.
    let (home, store) = setup();

    // Write a project config in the store root with a legacy `envs:` block.
    // The migrator must find and rewrite THIS file via project-config
    // discovery, with the process cwd set inside the repo.
    let config_path = store.path().join("himitsu.yaml");
    std::fs::write(
        &config_path,
        "default_store: org/repo\nenvs:\n  dev:\n    - dev/*\n    - DB_PASSWORD\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(store.path())
        .args(["--store", &store_flag(&store), "migrate", "envs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("output blocks rewritten: 1"));

    let migrated = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !migrated.contains("envs:"),
        "envs: should be gone from project config: {migrated}"
    );
    assert!(
        migrated.contains("codegen:"),
        "codegen: missing from project config: {migrated}"
    );
    assert!(
        migrated.contains("dev/*"),
        "selector dev/* missing: {migrated}"
    );
    // A backup of the original is written next to it, preserving the name.
    assert!(
        store.path().join("himitsu.yaml.bak").exists(),
        "expected himitsu.yaml.bak backup beside the migrated config"
    );
}

#[test]
fn migrate_envs_rewrites_project_config_via_explicit_project_flag() {
    // hm-j3s: `--project=<path>` invoked from a DIFFERENT cwd must still find
    // and rewrite the repo's project config (via Context.project_root), not
    // rely on a cwd walk. This exercises the explicit project-root path.
    let home = TempDir::new().unwrap();
    let slug = "myorg/migrated";
    create_remote_store(&home, slug);

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!("default_store: \"{slug}\"\nenvs:\n  dev:\n    - dev/*\n"),
    )
    .unwrap();

    // Run from an UNRELATED cwd, pointing at the project with --project=<path>.
    let elsewhere = tempfile::tempdir().unwrap();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(elsewhere.path())
        .args([
            &format!("--project={}", project_dir.path().to_string_lossy()),
            "migrate",
            "envs",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("output blocks rewritten: 1"));

    let migrated = std::fs::read_to_string(project_dir.path().join("himitsu.yaml")).unwrap();
    assert!(
        !migrated.contains("envs:"),
        "envs: should be gone: {migrated}"
    );
    assert!(
        migrated.contains("codegen:"),
        "codegen: missing: {migrated}"
    );
    assert!(
        migrated.contains("dev/*"),
        "selector dev/* missing: {migrated}"
    );
}

#[test]
fn setup_helper_creates_envelope_with_env_field() {
    let (_home, store, path) = setup_with_legacy_env_field("prod", None);
    let envelope_path = store
        .path()
        .join(".himitsu")
        .join("secrets")
        .join(format!("{}.age", path));
    assert!(
        envelope_path.exists(),
        "envelope file should exist at {:?}",
        envelope_path
    );
}

#[test]
fn fold_environment_to_tags_happy() {
    let (home, store, path) = setup_with_legacy_env_field("prod", None);
    let before = std::fs::read(legacy_envelope_path(&store, &path)).unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &store_flag(&store),
            "search",
            "",
            "--tag",
            "prod",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&path));

    let after = std::fs::read(legacy_envelope_path(&store, &path)).unwrap();
    assert_eq!(
        before, after,
        "folding legacy env must not rewrite the .age file"
    );
}

#[test]
fn fold_environment_deduplicates_existing_tag() {
    let (home, store, path) = setup_with_legacy_env_field("prod", Some("prod"));

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "get", &path])
        .assert()
        .success()
        .stdout("legacy-value")
        .stderr(predicate::str::contains("tags:        prod"))
        .stderr(predicate::str::contains("prod, prod").not());
}

#[test]
fn fold_does_not_mutate_disk() {
    let (home, store, path) = setup_with_legacy_env_field("prod", None);
    let before = std::fs::read(legacy_envelope_path(&store, &path)).unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "get", &path])
        .assert()
        .success()
        .stdout("legacy-value")
        .stderr(predicate::str::contains("tags:        prod"));

    let after = std::fs::read(legacy_envelope_path(&store, &path)).unwrap();
    assert_eq!(
        before, after,
        "himitsu get must not rewrite legacy envelopes"
    );
}

#[test]
fn fold_skips_invalid_env_value() {
    let (home, store, path) = setup_with_legacy_env_field("prod env with space", None);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "get", &path])
        .assert()
        .success()
        .stdout("legacy-value")
        .stderr(predicate::str::contains("tags:").not());

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &store_flag(&store),
            "search",
            "",
            "--tag",
            "prod-env-with-space",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&path).not());
}

fn legacy_envelope_path(store: &TempDir, path: &str) -> std::path::PathBuf {
    store
        .path()
        .join(".himitsu")
        .join("secrets")
        .join(format!("{path}.age"))
}

/// Returns the --store flag value for a given store root TempDir.
fn store_flag(store: &TempDir) -> String {
    store.path().to_string_lossy().to_string()
}

// ============ codegen (T18) tests — outputs-based input source ============

/// Language mode reads from `outputs:` config, not the store filesystem.
/// Output names become "environments" in the generated TypeScript.
#[test]
fn codegen_ts_from_outputs() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "dev/MY_SECRET", "hello"])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(
        project_dir.path(),
        "codegen:\n  pci-prod:\n    selectors:\n      - dev/MY_SECRET\n",
    );

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "codegen", "--lang", "typescript", "--stdout"])
        .current_dir(project_dir.path())
        .assert()
        .success()
        // Output name "pci-prod" must appear as an environment identifier.
        .stdout(predicate::str::contains("pci-prod"))
        // The resolved env_key for "dev/MY_SECRET" is "MY_SECRET".
        .stdout(predicate::str::contains("MY_SECRET"));
}

/// Language mode requires an `outputs:` block; bare project config gives an
/// actionable error.
#[test]
fn codegen_ts_requires_outputs_config() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(project_dir.path(), "{}\n");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "codegen", "--lang", "typescript", "--stdout"])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("codegen"));
}

/// `--env` flag filters to a specific output name; other outputs' keys are
/// excluded (unless `--merge-common` pulls in "common").
#[test]
fn codegen_ts_env_filter_selects_output() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/PROD_KEY", "pval"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "dev/DEV_KEY", "dval"])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(
        project_dir.path(),
        "codegen:\n  prod:\n    selectors:\n      - prod/PROD_KEY\n  dev:\n    selectors:\n      - dev/DEV_KEY\n",
    );

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "codegen",
            "--lang",
            "typescript",
            "--stdout",
            "--env",
            "prod",
        ])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("readonly prodKey: string"))
        .stdout(predicate::str::contains("readonly devKey: string").not());
}

// ============ init tests ============

#[test]
fn init_creates_directory_tree() {
    let (home, store) = setup();
    // Key files at data_dir (<home>/share/)
    assert!(home.path().join("share/key").exists());
    assert!(home.path().join("config.yaml").exists());
    // Store layout at store_root/.himitsu/
    assert!(store.path().join(".himitsu/secrets").exists());
    assert!(store.path().join(".himitsu/recipients").exists());
    assert!(store.path().join(".himitsu/recipients/self.pub").exists());
}

#[test]
fn init_is_idempotent() {
    let (home, store) = setup();
    let key_before = std::fs::read_to_string(home.path().join("share/key")).unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "init"])
        .assert()
        .success();

    let key_after = std::fs::read_to_string(home.path().join("share/key")).unwrap();
    assert_eq!(key_before, key_after);
}

#[test]
fn init_generates_valid_age_key() {
    let (home, _store) = setup();
    let contents = std::fs::read_to_string(home.path().join("share/key")).unwrap();
    assert!(contents.contains("AGE-SECRET-KEY-"));
    assert!(contents.contains("# public key: age1"));
}

#[test]
fn init_adds_self_as_recipient() {
    let (_home, store) = setup();
    let self_pub = store.path().join(".himitsu/recipients/self.pub");
    assert!(self_pub.exists());
    let contents = std::fs::read_to_string(self_pub).unwrap();
    assert!(contents.starts_with("age1"));
}

#[test]
fn init_name_creates_git_store_with_origin() {
    let home = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["init", "--name", "alice/secrets"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ Registered store alice/secrets (default)",
        ));

    let store = home.path().join("state/stores/alice/secrets");
    assert!(store.join(".himitsu/secrets").exists());
    assert!(store.join(".himitsu/recipients/self.pub").exists());
    assert!(store.join(".git").exists());

    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&store)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "git@github.com:alice/secrets.git"
    );

    let config = std::fs::read_to_string(home.path().join("config.yaml")).unwrap();
    assert!(config.contains("default_store: alice/secrets"));
}

#[test]
fn init_without_store_recommends_personal_github_store() {
    let home = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .env("GITHUB_USER", "alice")
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "himitsu init --name alice/secrets",
        ))
        .stdout(predicate::str::contains("primary personal GitHub store"));
}

#[test]
fn init_name_restores_and_pulls_existing_store() {
    let home = TempDir::new().unwrap();
    let source = tempfile::tempdir().unwrap();
    create_local_git_repo(source.path());
    commit_file(
        source.path(),
        ".himitsu/secrets/prod/API_KEY.yaml",
        b"ciphertext-v1\n",
        "add existing secret",
    );

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "init",
            "--name",
            "alice/secrets",
            "--url",
            &source.path().to_string_lossy(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ Restored store alice/secrets (default)",
        ));

    let restored = home.path().join("state/stores/alice/secrets");
    assert!(restored.join(".himitsu/secrets/prod/API_KEY.yaml").exists());

    commit_file(
        source.path(),
        ".himitsu/secrets/dev/NEW.yaml",
        b"ciphertext-v2\n",
        "add another secret",
    );

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "init",
            "--name",
            "alice/secrets",
            "--url",
            &source.path().to_string_lossy(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ Restored store alice/secrets (default)",
        ));

    assert!(restored.join(".himitsu/secrets/dev/NEW.yaml").exists());
}

// ============ set / get tests ============

#[test]
fn set_get_roundtrip() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/API_KEY", "secret123"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "get", "prod/API_KEY"])
        .assert()
        .success()
        .stdout("secret123");
}

#[test]
fn set_get_with_metadata_roundtrip() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "set",
            "prod/DB_PW",
            "s3cret",
            "--url",
            "https://db.example.com",
            "--totp",
            "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP",
            "--description",
            "Primary prod database",
            "--tag",
            "pci",
            "--tag",
            "prod",
            "--expires-at",
            "30d",
        ])
        .assert()
        .success();

    // stdout must still be the bare value for piping.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "get", "prod/DB_PW"])
        .assert()
        .success()
        .stdout("s3cret")
        .stderr(predicate::str::contains(
            "url:         https://db.example.com",
        ))
        .stderr(predicate::str::contains("totp:        JBSWY3DPEHPK3PXP"))
        .stderr(predicate::str::contains(
            "description: Primary prod database",
        ))
        .stderr(predicate::str::contains("tags:        pci, prod"))
        .stderr(predicate::str::contains("expires"));
}

#[test]
fn set_rejects_invalid_totp() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/BAD", "value", "--totp", "abc"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("totp"));
}

#[test]
fn set_rejects_invalid_expires_at() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "set",
            "prod/BAD",
            "value",
            "--expires-at",
            "30x",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown duration unit"));
}

#[test]
fn set_normalizes_leading_slash() {
    // /dev/hello is valid — leading / is stripped to give path dev/hello
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "/dev/hello", "world"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Set dev/hello"));

    // The age file lands at the normalized path, not any absolute location
    assert!(
        store
            .path()
            .join(".himitsu/secrets/dev/hello.yaml")
            .exists()
    );
}

#[test]
fn set_rejects_traversal_path() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "../../etc/passwd", "oops"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "not a valid secret path component",
        ));
}

#[test]
fn get_normalizes_leading_slash() {
    // /dev/hello resolves to the same secret as dev/hello
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "dev/hello", "world"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "get", "/dev/hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("world"));
}

#[test]
fn set_creates_age_file() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/DB_PASS", "hunter2"])
        .assert()
        .success();

    assert!(
        store
            .path()
            .join(".himitsu/secrets/prod/DB_PASS.yaml")
            .exists()
    );
}

#[test]
fn set_get_multiline_value() {
    let (home, store) = setup();
    let s = store_flag(&store);
    let multiline = "line1\nline2\nline3";

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/MULTI", multiline])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "get", "prod/MULTI"])
        .assert()
        .success()
        .stdout(multiline);
}

#[test]
fn set_get_special_characters() {
    let (home, store) = setup();
    let s = store_flag(&store);
    let special = r#"hello "world" \n back\slash"#;

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/SPECIAL", special])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "get", "prod/SPECIAL"])
        .assert()
        .success()
        .stdout(special);
}

// ============ ls tests ============

#[test]
fn ls_lists_secrets() {
    let (home, store) = setup();
    let s = store_flag(&store);

    for env in &["dev", "prod"] {
        himitsu()
            .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
            .args(["--store", &s, "set", &format!("{env}/KEY"), "val"])
            .assert()
            .success();
    }

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dev"))
        .stdout(predicate::str::contains("prod"));
}

#[test]
fn ls_filters_by_prefix() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/A_KEY", "a"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/B_KEY", "b"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "ls", "prod"])
        .assert()
        .success()
        .stdout(predicate::str::contains("A_KEY"))
        .stdout(predicate::str::contains("B_KEY"));
}

// ============ rekey (re-encrypt) tests ============

#[test]
fn rekey_re_encrypts_secrets() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/SECRET", "value"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "rekey"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Re-encrypted 1 secret"));

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "get", "prod/SECRET"])
        .assert()
        .success()
        .stdout("value");
}

// ============ recipient tests ============

#[test]
fn recipient_add_self() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "recipient",
            "add",
            "mydevice",
            "--self",
            "--description",
            "laptop",
        ])
        .assert()
        .success();

    let pub_file = store.path().join(".himitsu/recipients/mydevice.pub");
    assert!(pub_file.exists());
    // Sidecar should be written when --description is given.
    let sidecar = store.path().join(".himitsu/recipients/mydevice.yaml");
    assert!(sidecar.exists());
}

#[test]
fn recipient_add_explicit_key() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let key_txt = std::fs::read_to_string(home.path().join("share/key")).unwrap();
    let pubkey = key_txt
        .lines()
        .find(|l| l.starts_with("# public key: "))
        .unwrap()
        .strip_prefix("# public key: ")
        .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "recipient",
            "add",
            "bot",
            "--age-key",
            pubkey,
        ])
        .assert()
        .success();

    assert!(store.path().join(".himitsu/recipients/bot.pub").exists());
}

#[test]
fn recipient_rm() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "recipient", "add", "todelete", "--self"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "recipient", "rm", "todelete"])
        .assert()
        .success();

    assert!(
        !store
            .path()
            .join(".himitsu/recipients/todelete.pub")
            .exists()
    );
}

#[test]
fn recipient_ls() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "recipient", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("self"))
        .stdout(predicate::str::contains("NAME"));
}

// ============ group tests ============

// ============ search tests ============

#[test]
fn search_matches_keys() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/STRIPE_KEY", "sk_test"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "search", "STRIPE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE_KEY"));
}

#[test]
fn search_no_matches_returns_empty() {
    let (home, _store) = setup();

    // search without --store: scans stores_dir which is empty
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["search", "NONEXISTENT"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn search_typo_suggests_closest_match() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/api/STRIPE_KEY", "sk_test"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "search", "prod/api/STRIPE_KYE"])
        .assert()
        .success()
        .stderr(predicate::str::contains("did you mean prod/api/STRIPE_KEY"));
}

// ============ version and help tests ============

#[test]
fn version_prints_with_short_commit_sha_and_date() {
    himitsu()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("himitsu 0.1.0 (commit "))
        .stdout(
            predicate::str::is_match(r"(?:[0-9a-f]{7,}|unknown), (?:\d{4}-\d{2}-\d{2}|unknown)")
                .unwrap(),
        );
}

#[test]
fn version_subcommand_prints_without_initializing() {
    let home = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains("himitsu 0.1.0 (commit "))
        .stdout(
            predicate::str::is_match(r"(?:[0-9a-f]{7,}|unknown), (?:\d{4}-\d{2}-\d{2}|unknown)")
                .unwrap(),
        );

    assert!(!home.path().join("share/key").exists());
}

#[test]
fn help_shows_all_commands() {
    himitsu()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("search"))
        .stdout(predicate::str::contains("recipient"))
        .stdout(predicate::str::contains("rekey"))
        .stdout(predicate::str::contains("sync"))
        .stdout(predicate::str::contains("git"))
        .stdout(predicate::str::contains("generate"))
        .stdout(predicate::str::contains("remote"))
        .stdout(predicate::str::contains("ci"))
        .stdout(predicate::str::contains("ls"))
        .stdout(predicate::str::contains("version"));
}

#[test]
fn recipient_show_existing() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // Add a self recipient so there's something to find.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "recipient", "add", "mykey", "--self"])
        .assert()
        .success();

    // show should print the public key (plus metadata headers).
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "recipient", "show", "mykey"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Public key:"))
        .stdout(predicate::str::contains("age1"));
}

#[test]
fn recipient_show_nonexistent() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "recipient", "show", "nobody"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

// ============ --remote flag tests ============

/// Create and initialise a remote store at `<home>/state/stores/<org>/<repo>`.
/// This is where `--remote slug` looks for stores.
fn create_remote_store(home: &TempDir, slug: &str) -> std::path::PathBuf {
    let (org, repo) = slug.split_once('/').unwrap();
    let dest = home.path().join("state/stores").join(org).join(repo);
    std::fs::create_dir_all(&dest).unwrap();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &dest.to_string_lossy(), "init"])
        .assert()
        .success();
    dest
}

#[test]
fn remote_flag_resolves_to_stores_dir() {
    let (home, _store) = setup();
    let slug = "acme/secrets";
    let remote_store = create_remote_store(&home, slug);

    // Write a secret directly into the remote store.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &remote_store.to_string_lossy(),
            "set",
            "prod/REMOTE_KEY",
            "remote-value",
        ])
        .assert()
        .success();

    // Read it back via the short `-r` flag — must return the same value.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["-r", slug, "get", "prod/REMOTE_KEY"])
        .assert()
        .success()
        .stdout("remote-value");
}

#[test]
fn remote_flag_long_form_resolves() {
    let (home, _store) = setup();
    let slug = "myorg/myrepo";
    let remote_store = create_remote_store(&home, slug);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &remote_store.to_string_lossy(),
            "set",
            "dev/DB_PASS",
            "hunter2",
        ])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--remote", slug, "get", "dev/DB_PASS"])
        .assert()
        .success()
        .stdout("hunter2");
}

#[test]
fn remote_flag_fails_for_unknown_slug() {
    let (home, _store) = setup();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["-r", "ghost/missing", "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ghost/missing"));
}

#[test]
fn remote_flag_rejects_invalid_slug() {
    let (home, _store) = setup();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["-r", "notaslug", "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid"));
}

#[test]
fn remote_flag_conflicts_with_store() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // clap enforces mutual exclusion at parse time — must fail before dispatch.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "--remote", "org/repo", "ls"])
        .assert()
        .failure();
}

// ============ remote add tests ============

/// Create a local git repository with one initial commit so it can be cloned.
fn create_local_git_repo(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git command failed");
    };

    git(&["init"]);
    std::fs::write(path.join("README"), b"remote store\n").unwrap();
    git(&["add", "-A"]);
    git(&["-c", "commit.gpgsign=false", "commit", "-m", "init"]);
}

fn commit_file(path: &std::path::Path, relative_path: &str, contents: &[u8], message: &str) {
    let file_path = path.join(relative_path);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&file_path, contents).unwrap();

    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git command failed");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    git(&["add", relative_path]);
    git(&["-c", "commit.gpgsign=false", "commit", "-m", message]);
}

fn init_empty_git_checkout(path: &std::path::Path, origin: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("git command failed");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init"]);
    git(&["remote", "add", "origin", &origin.to_string_lossy()]);
}

#[test]
fn remote_add_clones_local_repo() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let source = tempfile::tempdir().unwrap();
    create_local_git_repo(source.path());

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "remote",
            "add",
            "test-org/my-repo",
            "--url",
            &source.path().to_string_lossy(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added remote 'test-org/my-repo'"));

    // The remote should be cloned into stores_dir = <home>/state/stores/
    assert!(home.path().join("state/stores/test-org/my-repo").exists());
}

#[test]
fn remote_add_pulls_existing_checkout() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let source = tempfile::tempdir().unwrap();
    create_local_git_repo(source.path());
    commit_file(
        source.path(),
        ".himitsu/secrets/prod/API_KEY.yaml",
        b"ciphertext\n",
        "add secret",
    );

    let dest = home.path().join("state/stores/test-org/my-repo");
    init_empty_git_checkout(&dest, source.path());

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "remote",
            "add",
            "test-org/my-repo",
            "--url",
            &source.path().to_string_lossy(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Updated remote 'test-org/my-repo'",
        ));

    assert!(dest.join(".himitsu/secrets/prod/API_KEY.yaml").exists());
}

#[test]
fn remote_add_resolves_via_remote_flag() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let source = tempfile::tempdir().unwrap();
    create_local_git_repo(source.path());

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "remote",
            "add",
            "acme/repo",
            "--url",
            &source.path().to_string_lossy(),
        ])
        .assert()
        .success();

    // After adding, `-r acme/repo` must not fail with "remote not found".
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["-r", "acme/repo", "ls"])
        .assert()
        .stderr(predicate::str::contains("remote not found").not());
}

#[test]
fn remote_add_invalid_slug_fails() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "add", "notaslug"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid"));
}

// ============ remote default/list/remove tests ============

#[test]
fn remote_list_shows_all() {
    let (home, store) = setup();
    let s = store_flag(&store);
    create_remote_store(&home, "acme/secrets");
    create_remote_store(&home, "myorg/keys");
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acme/secrets"))
        .stdout(predicate::str::contains("myorg/keys"));
}

#[test]
fn remote_list_empty() {
    let (home, store) = setup();
    let s = store_flag(&store);
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no remotes found"));
}

#[test]
fn remote_default_shows_none() {
    let (home, store) = setup();
    let s = store_flag(&store);
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "default"])
        .assert()
        .success()
        .stdout(predicate::str::contains("none set"));
}

#[test]
fn remote_default_sets_and_shows() {
    let (home, store) = setup();
    let s = store_flag(&store);
    // Set a default store slug.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "default", "acme/secrets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Default store set to"));
    // Read it back — should echo the slug.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "default"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acme/secrets"));
}

#[test]
fn remote_remove_deletes_checkout() {
    let (home, store) = setup();
    let s = store_flag(&store);
    let remote_path = create_remote_store(&home, "acme/vault");
    assert!(remote_path.exists());
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "remove", "acme/vault"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed remote"));
    assert!(!remote_path.exists());
}

#[test]
fn remote_remove_nonexistent_fails() {
    let (home, store) = setup();
    let s = store_flag(&store);
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "remove", "ghost/missing"])
        .assert()
        .failure();
}

#[test]
fn remote_remove_clears_default() {
    let (home, store) = setup();
    let s = store_flag(&store);
    create_remote_store(&home, "acme/vault");
    // Set acme/vault as the default.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "default", "acme/vault"])
        .assert()
        .success();
    // Remove the store.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "remove", "acme/vault"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Cleared default store"));
    // Default should now be "none set".
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "remote", "default"])
        .assert()
        .success()
        .stdout(predicate::str::contains("none set"));
}

// ============ sync tests ============

#[test]
fn sync_no_stores_shows_hint() {
    // With no remote stores, sync prints a helpful message and exits 0.
    // Note: no --store flag; sync resolves stores independently via list_remotes().
    let (home, _store) = setup();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no stores found"));
}

#[test]
fn sync_specific_store() {
    // Syncing a named store slug succeeds and mentions it in output.
    let (home, store) = setup();
    let s = store_flag(&store);
    create_remote_store(&home, "acme/vault");
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "sync", "acme/vault"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acme/vault"));
}

#[test]
fn sync_no_rekey_flag_skips_rekey() {
    // With --no-rekey, output mentions the slug and does NOT say "rekeyed".
    // The pull stage is best-effort and may print "pulled" or "skipped pull"
    // depending on whether the test's local store has a real origin remote.
    let (home, store) = setup();
    let s = store_flag(&store);
    create_remote_store(&home, "acme/vault");
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "sync", "acme/vault", "--no-rekey"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acme/vault"))
        .stdout(predicate::str::contains("rekeyed").not());
}

#[test]
fn sync_invalid_slug_fails() {
    // A slug without an org/repo separator is rejected.
    let (home, store) = setup();
    let s = store_flag(&store);
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "sync", "notaslug"])
        .assert()
        .failure();
}

// ============ git tests ============

/// Initialise a git repo inside an existing directory so it can be used as a
/// store checkout in git-subcommand tests.
fn init_git_repo(path: &std::path::Path) {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git command failed");
    };
    git(&["init"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-m",
        "init",
        "--allow-empty",
    ]);
}

#[test]
fn git_help_shows_usage() {
    himitsu()
        .args(["git", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Run git commands inside a store checkout",
        ));
}

#[test]
fn git_store_scoped_status() {
    // --remote resolves to the store checkout; git status should succeed.
    let (home, _store) = setup();
    let remote = create_remote_store(&home, "acme/vault");
    init_git_repo(&remote);
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--remote", "acme/vault", "git", "status"])
        .assert()
        .success();
}

#[test]
fn git_defaults_to_status() {
    // Running `himitsu git` with no sub-command defaults to `git status`.
    let (home, _store) = setup();
    let remote = create_remote_store(&home, "acme/vault");
    init_git_repo(&remote);
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--remote", "acme/vault", "git"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("branch").or(predicate::str::contains("nothing to commit")),
        );
}

#[test]
fn git_all_runs_in_each_store() {
    // --all iterates all registered stores and prints a banner for each.
    let (home, _store) = setup();
    let r1 = create_remote_store(&home, "acme/vault");
    let r2 = create_remote_store(&home, "myorg/keys");
    init_git_repo(&r1);
    init_git_repo(&r2);
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["git", "--all", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("=== acme/vault ==="))
        .stdout(predicate::str::contains("=== myorg/keys ==="));
}

#[test]
fn git_no_store_no_all_errors() {
    // Without --remote, --store, or --all, git returns a clear error.
    let (home, _store) = setup();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["git", "status"])
        .assert()
        .failure();
}

#[test]
fn git_passthrough_respects_flags() {
    // Extra git flags (e.g. --oneline) are forwarded to git.
    let (home, _store) = setup();
    let remote = create_remote_store(&home, "acme/vault");
    init_git_repo(&remote);
    // `git log` on a repo with one commit should succeed and show the message.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--remote", "acme/vault", "git", "log", "--oneline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("init"));
}

// ============ auto-init (first-use) tests ============

#[test]
fn auto_init_on_first_command() {
    // Running any non-init command without a key file should automatically
    // initialize — no prompt, no stdin.
    let home = TempDir::new().unwrap();
    let fake_home = home.path().join("fresh");
    let cwd = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", fake_home.join("config.yaml"))
        .current_dir(cwd.path())
        .args(["ls"])
        .assert()
        .success();

    // Verify init happened — key file should exist now.
    assert!(fake_home.join("share/key").exists());
}

#[test]
fn auto_init_prints_notice_to_stderr() {
    let home = TempDir::new().unwrap();
    let fake_home = home.path().join("brand_new");
    let cwd = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", fake_home.join("config.yaml"))
        .current_dir(cwd.path())
        .args(["ls"])
        .assert()
        .success()
        .stderr(predicate::str::contains("First run"));
}

#[test]
fn auto_init_then_command_continues() {
    // After auto-init, the original command (ls) should still execute.
    let home = TempDir::new().unwrap();
    let fake_home = home.path().join("new");
    let cwd = TempDir::new().unwrap();

    // ls with no stores: returns 0 and prints nothing (empty).
    himitsu()
        .env("HIMITSU_CONFIG", fake_home.join("config.yaml"))
        .current_dir(cwd.path())
        .args(["ls"])
        .assert()
        .success();
}

// ============ explicit --store creation prompt tests ============

#[test]
fn explicit_store_prompt_creates_store_on_accept() {
    let home = TempDir::new().unwrap();
    let store = home.path().join("new-store");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["init"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .write_stdin("\n")
        .args(["--store", &store.to_string_lossy(), "ls"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "No store exists. Create one at {}? Y/n",
            store.display()
        )))
        .stderr(predicate::str::contains("No secrets found"));

    assert!(store.join(".himitsu/secrets").exists());
    assert!(store.join(".himitsu/recipients/self.pub").exists());
}

#[test]
fn explicit_store_prompt_aborts_on_decline() {
    let home = TempDir::new().unwrap();
    let store = home.path().join("declined-store");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["init"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .write_stdin("n\n")
        .args(["--store", &store.to_string_lossy(), "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "No store exists. Create one at {}? Y/n",
            store.display()
        )))
        .stderr(predicate::str::contains("store creation declined"));

    assert!(!store.join(".himitsu").exists());
}

// ============ init wizard output tests ============

#[test]
fn init_shows_wizard_output_on_first_run() {
    let home = TempDir::new().unwrap();

    // First-ever init: wizard summary.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Created age keypair"))
        .stdout(predicate::str::contains("Public key: age1"))
        .stdout(predicate::str::contains("✓ Created state directory"));
}

#[test]
fn init_idempotent_shows_already_initialized() {
    let (home, store) = setup();

    // Second run: already-initialized message.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Already initialized."))
        .stdout(predicate::str::contains("Public key: age1"));
}

#[test]
fn init_with_name_registers_store_as_default() {
    let home = TempDir::new().unwrap();
    let slug = "myorg/myproject";

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["init", "--name", slug])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ Registered store myorg/myproject (default)",
        ));

    // The store directory should exist under stores_dir.
    assert!(home.path().join("state/stores/myorg/myproject").exists());

    // The global config should have default_store set.
    let cfg_text = std::fs::read_to_string(home.path().join("config.yaml")).unwrap();
    assert!(cfg_text.contains("myorg/myproject"));
}

// ============ lazy-clone error tests ============

#[test]
fn lazy_clone_failure_shows_helpful_tip() {
    // When a slug is given via --remote and the store doesn't exist locally,
    // himitsu attempts a lazy git clone. For a non-existent repo the clone
    // fails, and the error should contain a helpful tip.
    let (home, _store) = setup();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["-r", "ghost/nonexistent", "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ghost/nonexistent"))
        .stderr(predicate::str::contains("Tip"));
}

// ============ default-store resolution tests ============

#[test]
fn resolve_store_via_global_config_default() {
    // When only one default_store is set in global config, resolve_store
    // should pick it up without --remote.
    let (home, _store) = setup();
    let slug = "acme/cfg";
    create_remote_store(&home, slug);

    // Set the default store in global config.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["remote", "default", slug])
        .assert()
        .success();

    // Now `ls` should succeed without any --remote or --store flag.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["ls"])
        .assert()
        .success();
}

#[test]
fn resolve_store_via_project_config() {
    let (home, _store) = setup();
    let slug = "myorg/projected";
    create_remote_store(&home, slug);

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!("default_store: \"{slug}\"\n"),
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(project_dir.path())
        .args(["--project", "ls"])
        .assert()
        .success();
}

#[test]
fn resolve_store_remote_flag_overrides_project_config() {
    // --remote must always win over both project and global config.
    let (home, _store) = setup();
    let project_slug = "myorg/projected";
    let override_slug = "myorg/override";
    create_remote_store(&home, project_slug);
    create_remote_store(&home, override_slug);

    let project_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!("default_store: \"{project_slug}\"\n"),
    )
    .unwrap();

    // With --remote, the override store should be used (not the project config one).
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(project_dir.path())
        .args(["-r", override_slug, "ls"])
        .assert()
        .success();
}

#[test]
fn resolve_store_project_config_over_global_config() {
    let (home, _store) = setup();
    let global_slug = "myorg/global";
    let project_slug = "myorg/local";
    create_remote_store(&home, global_slug);
    create_remote_store(&home, project_slug);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["remote", "default", global_slug])
        .assert()
        .success();

    let project_path = home.path().join("state/stores/myorg/local");
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &project_path.to_string_lossy(),
            "set",
            "prod/MARKER",
            "from-project",
        ])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!("default_store: \"{project_slug}\"\n"),
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(project_dir.path())
        .args(["--project", "get", "prod/MARKER"])
        .assert()
        .success()
        .stdout("from-project");
}

#[test]
fn resolve_store_single_implicit() {
    // With exactly one store registered and no default set, that store should
    // be selected automatically for commands that require a store.
    let (home, _store) = setup();
    create_remote_store(&home, "sole/store");

    // `rekey` is in `needs_store` and is safe to run (no-ops on an empty store).
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["rekey"])
        .assert()
        .success();
}

#[test]
fn resolve_store_ambiguous_error_is_actionable() {
    // Multiple stores and no default → commands that require a store must fail
    // with an error that names the stores and suggests --remote / `remote default`.
    let (home, _store) = setup();
    create_remote_store(&home, "acme/prod");
    create_remote_store(&home, "acme/staging");

    // `get` is in `needs_store`, so it calls resolve_store and hits the
    // ambiguous-store error when no default is configured.
    let output = himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["get", "prod/SOME_KEY"])
        .assert()
        .failure();

    // Error message should name the stores and give actionable guidance.
    output
        .stderr(predicate::str::contains("acme/prod").or(predicate::str::contains("acme/staging")))
        .stderr(
            predicate::str::contains("--remote").or(predicate::str::contains("remote default")),
        );
}

// ============ generate command tests ============

/// Write a `himitsu.yaml` in the given directory with the supplied YAML content.
fn write_project_config(dir: &std::path::Path, yaml: &str) {
    std::fs::write(dir.join("himitsu.yaml"), yaml).unwrap();
}

#[test]
fn generate_basic_stdout() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(project_dir.path(), "{}\n");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "generate", "--stdout", "--output", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("codegen"));
}

#[test]
fn generate_alias_entry() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(project_dir.path(), "{}\n");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "generate", "--stdout", "--output", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("codegen"));
}

#[test]
fn generate_glob_entry() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(project_dir.path(), "{}\n");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "generate", "--stdout", "--output", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("codegen"));
}

#[test]
fn generate_single_entry_only_that_key() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(project_dir.path(), "{}\n");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "generate", "--stdout", "--output", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("codegen"));
}

#[test]
fn generate_no_project_config_errors() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // Run from a directory that has no himitsu.yaml anywhere above it.
    let empty_dir = tempfile::tempdir().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "generate", "--stdout"])
        .current_dir(empty_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("project config"));
}

#[test]
fn generate_unknown_env_errors() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(project_dir.path(), "{}\n");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "generate",
            "--stdout",
            "--output",
            "nonexistent",
        ])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("codegen"));
}

#[test]
fn project_config_discovers_dotconfig_variant() {
    let (home, _store) = setup();

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    let config_subdir = project_dir.path().join(".config");
    std::fs::create_dir_all(&config_subdir).unwrap();
    std::fs::write(config_subdir.join("himitsu.yaml"), "{}\n").unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--project", "ls"])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("default_store"));
}

#[test]
fn project_config_discovers_dothimitsu_variant() {
    let (home, _store) = setup();

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    let himitsu_subdir = project_dir.path().join(".himitsu");
    std::fs::create_dir_all(&himitsu_subdir).unwrap();
    std::fs::write(himitsu_subdir.join("config.yaml"), "{}\n").unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--project", "ls"])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("default_store"));
}

#[test]
fn generate_all_envs_stdout() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(project_dir.path(), "{}\n");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "generate", "--stdout"])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("codegen"));
}

#[test]
fn generate_output_flag_works() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "dev/MY_SECRET", "hello123"])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(
        project_dir.path(),
        "codegen:\n  pci-prod:\n    selectors:\n      - dev/MY_SECRET\n",
    );

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store", &s, "generate", "--stdout", "--output", "pci-prod",
        ])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("MY_SECRET"));
}

#[test]
fn generate_tag_selector_output_resolves() {
    // hm-5i3 / Oracle: `generate` must resolve `tag:` selectors in outputs:.
    // Previously generate built candidates with empty tags, so tag selectors
    // matched nothing and produced empty output.
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "set",
            "prod/STRIPE_KEY",
            "sk_live_gen",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(
        project_dir.path(),
        "codegen:\n  pci-prod:\n    selectors:\n      - tag:pci\n",
    );

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store", &s, "generate", "--stdout", "--output", "pci-prod",
        ])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE_KEY"));
}

#[test]
fn generate_tag_selector_alias_resolves() {
    // hm-5i3 / Oracle: `generate` must resolve a selector-valued alias
    // (`STRIPE: tag:stripe`) to its single matching secret under the alias key.
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "set",
            "prod/stripe-key",
            "sk_gen_alias",
            "--tag",
            "stripe",
        ])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(
        project_dir.path(),
        "codegen:\n  app:\n    selectors: []\n    aliases:\n      STRIPE: tag:stripe\n",
    );

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "generate", "--stdout", "--output", "app"])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE"));
}

#[test]
fn generate_env_flag_hard_errors() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(project_dir.path(), "{}\n");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "generate", "--stdout", "--env", "pci-prod"])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--env flag has been removed"));
}

// ============ provider-prefixed path tests ============

/// Set up a named store at `stores_dir/org/repo` using `himitsu init --name`.
/// Returns the home TempDir with keys and config.
fn setup_named_store(home: &TempDir, slug: &str) {
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["init", "--name", slug])
        .assert()
        .success();
}

#[test]
fn get_with_qualified_provider_prefix() {
    let home = TempDir::new().unwrap();

    // Register a named store and set it as default.
    setup_named_store(&home, "acme/secrets");

    // Store a secret in the named store via --remote.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--remote",
            "acme/secrets",
            "set",
            "prod/API_KEY",
            "mysecret123",
        ])
        .assert()
        .success();

    // Retrieve via provider-prefixed qualified ref (no --remote needed).
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["get", "github:acme/secrets/prod/API_KEY"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mysecret123"));
}

#[test]
fn set_with_qualified_provider_prefix() {
    let home = TempDir::new().unwrap();

    setup_named_store(&home, "acme/secrets");

    // Store via provider-prefixed qualified ref.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["set", "github:acme/secrets/staging/DB_PASS", "dbpass456"])
        .assert()
        .success();

    // Verify by reading back with --remote.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--remote", "acme/secrets", "get", "staging/DB_PASS"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dbpass456"));
}

#[test]
fn ls_with_qualified_provider_prefix_lists_store_secrets() {
    let home = TempDir::new().unwrap();

    setup_named_store(&home, "acme/secrets");

    // Populate a few secrets.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--remote", "acme/secrets", "set", "prod/API_KEY", "v1"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--remote", "acme/secrets", "set", "dev/API_KEY", "v2"])
        .assert()
        .success();

    // `ls github:acme/secrets` → depth 1 by default, so subdirectories are
    // collapsed: shows "dev/" and "prod/" but not their children.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["ls", "github:acme/secrets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prod/"))
        .stdout(predicate::str::contains("dev/"));

    // With -R (recursive) all leaf paths are visible.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["ls", "-R", "github:acme/secrets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("API_KEY"));
}

#[test]
fn ls_with_qualified_prefix_filters_secrets() {
    let home = TempDir::new().unwrap();

    setup_named_store(&home, "acme/secrets");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--remote", "acme/secrets", "set", "prod/API_KEY", "v1"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--remote", "acme/secrets", "set", "dev/API_KEY", "v2"])
        .assert()
        .success();

    // `ls github:acme/secrets/prod` → prefix "prod", depth 1 relative to it.
    // prod/API_KEY is one level under prod/ so it shows as a leaf.
    // dev/* must be absent.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["ls", "github:acme/secrets/prod"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prod/API_KEY"))
        .stdout(predicate::str::contains("dev").not());
}

#[test]
fn get_qualified_unknown_store_returns_error() {
    let home = TempDir::new().unwrap();
    // Init home (keys only, no named store).
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["init"])
        .assert()
        .success();

    // Requesting a store that doesn't exist should error.
    // We need at least one store registered so resolve_store doesn't fail
    // before we reach the command handler. Register a dummy store.
    setup_named_store(&home, "dummy/store");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["get", "github:no/such/prod/KEY"])
        .assert()
        .failure();
}

// ============ store.recipients_path tests ============

#[test]
fn set_and_get_with_custom_recipients_path() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // Move the default recipients directory to a custom location.
    let default_recipients = store.path().join(".himitsu/recipients");
    let custom_dir = store.path().join("custom/recipients");
    std::fs::create_dir_all(custom_dir.parent().unwrap()).unwrap();
    std::fs::rename(&default_recipients, &custom_dir).unwrap();

    // Write a store-internal config pointing to the custom path.
    let store_config = store.path().join(".himitsu/config.yaml");
    std::fs::write(&store_config, "recipients_path: custom/recipients\n").unwrap();

    // `set` should find recipients in the custom directory.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "prod/TOKEN", "value123"])
        .assert()
        .success();

    // `get` should decrypt the secret successfully.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "get", "prod/TOKEN"])
        .assert()
        .success()
        .stdout(predicate::str::contains("value123"));
}

#[test]
fn rekey_with_custom_recipients_path() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // First set a secret using the default path.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "set", "dev/SECRET", "rekey_me"])
        .assert()
        .success();

    // Move recipients to a custom path.
    let default_recipients = store.path().join(".himitsu/recipients");
    let custom_dir = store.path().join("alt/recips");
    std::fs::create_dir_all(custom_dir.parent().unwrap()).unwrap();
    std::fs::rename(&default_recipients, &custom_dir).unwrap();

    let store_config = store.path().join(".himitsu/config.yaml");
    std::fs::write(&store_config, "recipients_path: alt/recips\n").unwrap();

    // `rekey` should work with the custom recipients path.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "rekey"])
        .assert()
        .success();

    // `get` should still decrypt correctly after rekey.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "get", "dev/SECRET"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rekey_me"));
}

#[test]
fn recipient_add_with_custom_recipients_path() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // Move recipients to a custom path.
    let default_recipients = store.path().join(".himitsu/recipients");
    let custom_dir = store.path().join("my/recipients");
    std::fs::create_dir_all(custom_dir.parent().unwrap()).unwrap();
    std::fs::rename(&default_recipients, &custom_dir).unwrap();

    let store_config = store.path().join(".himitsu/config.yaml");
    std::fs::write(&store_config, "recipients_path: my/recipients\n").unwrap();

    // Add a recipient — should land in the custom directory.
    let (_, pub_key) = {
        let key_file = home.path().join("share/key");
        let contents = std::fs::read_to_string(&key_file).unwrap();
        let pubkey = contents
            .lines()
            .find(|l| l.starts_with("# public key: "))
            .unwrap()
            .strip_prefix("# public key: ")
            .unwrap()
            .trim()
            .to_string();
        ("", pubkey)
    };

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &s,
            "recipient",
            "add",
            "extra-key",
            "--age-key",
            &pub_key,
        ])
        .assert()
        .success();

    // The new key file should be under the custom path (flat layout).
    assert!(store.path().join("my/recipients/extra-key.pub").exists());
}

// ============ check tests ============

/// Configure git identity for commits in a directory.
fn git_config_identity(dir: &std::path::Path) {
    for (k, v) in &[("user.email", "test@example.com"), ("user.name", "Test")] {
        std::process::Command::new("git")
            .args(["config", k, v])
            .current_dir(dir)
            .output()
            .unwrap();
    }
}

/// Create a bare repo + clone pair that are in sync.
/// Returns `(bare_dir, stores_dir_path)` — the stores_dir is created at
/// `state/stores/<org>/<repo>` inside the given `home`.
fn setup_synced_store(home: &tempfile::TempDir, org: &str, repo: &str) -> tempfile::TempDir {
    let bare = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init", "--bare", bare.path().to_str().unwrap()])
        .output()
        .unwrap();

    let stores_dir = home.path().join(format!("state/stores/{org}/{repo}"));
    std::process::Command::new("git")
        .args([
            "clone",
            bare.path().to_str().unwrap(),
            stores_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    git_config_identity(&stores_dir);

    std::fs::write(stores_dir.join("README.md"), "init\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&stores_dir)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(&stores_dir)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["push", "origin", "HEAD"])
        .current_dir(&stores_dir)
        .output()
        .unwrap();

    bare
}

/// Push one more commit to the bare repo (via a second clone) so the main
/// checkout is behind.  Then fetch in the main checkout so remote refs update.
fn make_store_behind(bare: &tempfile::TempDir, stores_dir: &std::path::Path) {
    let second = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git")
        .args([
            "clone",
            bare.path().to_str().unwrap(),
            second.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    git_config_identity(second.path());
    std::fs::write(second.path().join("extra.txt"), "extra\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(second.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "extra"])
        .current_dir(second.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["push", "origin", "HEAD"])
        .current_dir(second.path())
        .output()
        .unwrap();

    // Update remote refs in the main checkout
    std::process::Command::new("git")
        .args(["fetch", "--quiet"])
        .current_dir(stores_dir)
        .output()
        .unwrap();
}

#[test]
fn test_check_up_to_date() {
    let (home, _store) = setup();

    let _bare = setup_synced_store(&home, "myorg", "mypkg");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["check", "myorg/mypkg", "--offline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("up to date"));
}

#[test]
fn test_check_behind_exits_nonzero() {
    let (home, _store) = setup();

    let bare = setup_synced_store(&home, "myorg", "behind");
    let stores_dir = home.path().join("state/stores/myorg/behind");
    make_store_behind(&bare, &stores_dir);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["check", "myorg/behind", "--offline"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("behind origin"));
}

#[test]
fn test_check_no_stores() {
    let (home, _store) = setup();

    // No stores in stores_dir → should print a message and exit 0.
    // (setup() creates a --store-based init, not a slug-based remote store.)
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no stores found"));
}

#[test]
fn test_check_offline_skips_fetch() {
    let (home, _store) = setup();

    let stores_dir = home.path().join("state/stores/myorg/offline");
    std::fs::create_dir_all(&stores_dir).unwrap();

    // Init a local git repo with an unreachable remote URL.
    std::process::Command::new("git")
        .args(["init", stores_dir.to_str().unwrap()])
        .output()
        .unwrap();
    git_config_identity(&stores_dir);
    std::fs::write(stores_dir.join("README.md"), "hello\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&stores_dir)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(&stores_dir)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://does-not-exist.invalid/org/repo.git",
        ])
        .current_dir(&stores_dir)
        .output()
        .unwrap();

    // --offline: fetch is skipped, no network error.  No remote tracking branch
    // means the command prints a warning about no tracking branch but exits 0.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["check", "myorg/offline", "--offline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("myorg/offline"));
}

// ============ read / write tests ============

#[test]
fn write_and_read_roundtrip() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "write", "prod/TOKEN", "abc123"])
        .assert()
        .success()
        .stdout("");

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "read", "prod/TOKEN"])
        .assert()
        .success()
        .stdout("abc123");
}

#[test]
fn write_reads_from_stdin_when_no_value() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "write", "prod/FROM_STDIN"])
        .write_stdin("piped-value")
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &s, "read", "prod/FROM_STDIN"])
        .assert()
        .success()
        .stdout("piped-value");
}

// ============ completions tests ============

#[test]
fn completions_bash_outputs_script() {
    himitsu()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("himitsu"));
}

// ============ ci tests ============

#[test]
fn ci_status_reports_missing_workflow_without_initializing() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(project.path())
        .args(["ci", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not installed"));

    assert!(!home.path().join("share/key").exists());
}

#[test]
fn ci_install_writes_github_actions_workflow() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(project.path())
        .args(["ci", "install", "--default-remote", "acme/secrets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed"));

    let workflow = project.path().join(".github/workflows/himitsu.yml");
    let contents = std::fs::read_to_string(workflow).unwrap();
    assert!(contents.contains("Himitsu Self-Serve Rekey"));
    assert!(contents.contains("darkmatter/himitsu"));
    assert!(contents.contains("default: acme/secrets"));
}

#[test]
fn ci_run_dry_run_prints_workflow_command() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(project.path())
        .args([
            "ci",
            "run",
            "--dry-run",
            "--operation",
            "add-recipient",
            "--target-remote",
            "acme/secrets",
            "--recipient-name",
            "alice",
            "--recipient-key",
            "age1example",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("gh workflow run"))
        .stdout(predicate::str::contains("operation=add-recipient"))
        .stdout(predicate::str::contains("remote=acme/secrets"));
}

// ============ --project context tests (hm-9zc.1) ============

#[test]
fn project_bare_resolves_git_root_from_cwd() {
    let (home, _store) = setup();
    let slug = "myorg/projected";
    create_remote_store(&home, slug);

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!("default_store: \"{slug}\"\n"),
    )
    .unwrap();

    let nested = project_dir.path().join("sub/dir");
    std::fs::create_dir_all(&nested).unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(&nested)
        .args(["--project", "ls"])
        .assert()
        .success();
}

#[test]
fn project_explicit_path_resolves_that_repo() {
    let (home, _store) = setup();
    let slug = "myorg/explicit";
    create_remote_store(&home, slug);

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!("default_store: \"{slug}\"\n"),
    )
    .unwrap();

    let elsewhere = tempfile::tempdir().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(elsewhere.path())
        .args([&format!("--project={}", project_dir.path().display()), "ls"])
        .assert()
        .success();
}

#[test]
fn project_outside_git_repo_errors_with_guidance() {
    let (home, _store) = setup();

    let not_a_repo = tempfile::tempdir().unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(not_a_repo.path())
        .args(["--project", "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--project requires a git repository",
        ));
}

#[test]
fn project_in_repo_without_config_errors_with_setup_hint() {
    let (home, _store) = setup();

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(project_dir.path())
        .args(["--project", "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no project config"))
        .stderr(predicate::str::contains("himitsu init --project"));
}

#[test]
fn project_config_without_default_store_errors() {
    let (home, _store) = setup();

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("himitsu.yaml"), "{}\n").unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(project_dir.path())
        .args(["--project", "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no `default_store` set"));
}

#[test]
fn no_project_flag_does_not_silently_switch_to_project_store() {
    let (home, _store) = setup();
    let global_slug = "myorg/global";
    let project_slug = "myorg/local";
    create_remote_store(&home, global_slug);
    create_remote_store(&home, project_slug);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["remote", "default", global_slug])
        .assert()
        .success();

    let global_path = home.path().join("state/stores/myorg/global");
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &global_path.to_string_lossy(),
            "set",
            "prod/MARKER",
            "from-global",
        ])
        .assert()
        .success();

    let project_path = home.path().join("state/stores/myorg/local");
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &project_path.to_string_lossy(),
            "set",
            "prod/MARKER",
            "from-project",
        ])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!("default_store: \"{project_slug}\"\n"),
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(project_dir.path())
        .args(["get", "prod/MARKER"])
        .assert()
        .success()
        .stdout("from-global");
}

#[test]
fn project_flag_conflicts_with_store() {
    let (home, store) = setup();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--project", "--store", &store_flag(&store), "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn project_flag_conflicts_with_remote() {
    let (home, _store) = setup();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--project", "-r", "any/slug", "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn store_flag_still_works_as_compat_escape_hatch() {
    let (home, store) = setup();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "ls"])
        .assert()
        .success();
}

// ============ config envs hard-error + outputs tests ============

#[test]
fn config_envs_key_warns_but_does_not_hard_error() {
    let (home, store) = setup();
    let cfg_path = home.path().join("config.yaml");
    std::fs::write(&cfg_path, "envs:\n  dev:\n    - dev/API_KEY\n").unwrap();

    // The legacy `envs:` key now deserializes successfully (emitting a stderr
    // warning) instead of hard-rejecting, so the migrate command can run.
    himitsu()
        .env("HIMITSU_CONFIG", &cfg_path)
        .args(["--store", &store_flag(&store), "ls"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "run 'himitsu migrate envs' to convert",
        ));
}

#[test]
fn config_codegen_key_parses_ok() {
    let (home, store) = setup();
    let cfg_path = home.path().join("config.yaml");
    std::fs::write(
        &cfg_path,
        "codegen:\n  pci-prod:\n    selectors:\n      - tag:pci\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg_path)
        .args(["--store", &store_flag(&store), "ls"])
        .assert()
        .success();
}

#[test]
fn config_with_legacy_outputs_key_errors_with_rename_guidance() {
    // hm-66a: the `outputs:` config key was hard-renamed to `codegen:`.
    // A non-empty legacy `outputs:` block must fail typed config loads
    // with guidance instead of being silently ignored. Use a command that
    // resolves the store from config (no `--store` override) so the global
    // config loads with error propagation (with `--store`, only best-effort
    // loads run and errors are swallowed by design).
    let (home, _store) = setup();
    let cfg_path = home.path().join("config.yaml");
    std::fs::write(
        &cfg_path,
        "default_store: org/secrets\noutputs:\n  pci-prod:\n    selectors:\n      - tag:pci\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .env("HIMITSU_CONFIG", &cfg_path)
        .args(["get", "prod/x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("renamed to 'codegen:'"));
}

#[test]
fn migrate_envs_renames_outputs_key_to_codegen() {
    // hm-66a: `himitsu migrate envs` must also rename a legacy top-level
    // `outputs:` block to `codegen:` (value preserved verbatim), and the
    // converted config must load cleanly afterwards.
    let (home, store) = setup();
    let config_path = store.path().join(".himitsu.yaml");
    std::fs::write(
        &config_path,
        "outputs:\n  pci-prod:\n    selectors:\n      - tag:pci\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args(["--store", &store_flag(&store), "migrate", "envs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("output blocks rewritten: 1"));

    let migrated = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !migrated.contains("outputs:"),
        "outputs: should be gone: {migrated}"
    );
    assert!(
        migrated.contains("codegen:"),
        "codegen: missing: {migrated}"
    );
    assert!(
        migrated.contains("tag:pci"),
        "selector must be preserved verbatim: {migrated}"
    );

    // The converted config now loads cleanly (no hard error).
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(store.path())
        .args(["--store", &store_flag(&store), "ls"])
        .assert()
        .success();
}

// ============ exec tag-selector tests (T15) ============

#[test]
fn exec_tag_selector_injects_tagged_secrets_only() {
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    // prod/api-key tagged "prod"
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/api-key",
            "sk_prod_123",
            "--tag",
            "prod",
        ])
        .assert()
        .success();

    // prod/db-pass NOT tagged
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/db-pass",
            "hunter2",
        ])
        .assert()
        .success();

    // dev/token tagged "dev" only
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "dev/token",
            "dev_tok_456",
            "--tag",
            "dev",
        ])
        .assert()
        .success();

    // exec tag:prod -- env  →  only prod/api-key → API_KEY injected
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "tag:prod",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("API_KEY=sk_prod_123"))
        .stdout(predicate::str::contains("DB_PASS").not())
        .stdout(predicate::str::contains("TOKEN=dev_tok_456").not());
}

#[test]
fn exec_output_label_resolves_tag_selector() {
    // Regression for hm-5i3: `himitsu exec <output-label>` is documented
    // (README) but exec previously parsed the label as a concrete path and
    // matched nothing. exec must resolve an `outputs:` label from project
    // config, including tag selectors inside it.
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/stripe-key",
            "sk_live_pci",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    // Define a `pci-prod` output whose selector is `tag:pci`, in a project
    // config at the store root. exec loads it via ctx.project_config() (cwd
    // walk when no --project root is set), so we run with cwd set to the
    // store root.
    let project_cfg = store.path().join("himitsu.yaml");
    std::fs::write(
        &project_cfg,
        "codegen:\n  pci-prod:\n    selectors:\n      - tag:pci\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "pci-prod",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE_KEY=sk_live_pci"));
}

#[test]
fn exec_output_label_resolves_via_explicit_project_flag() {
    // hm-5i3 / Oracle: `--project=<path> exec <output-label>` invoked from an
    // UNRELATED cwd must resolve the label from the project root (via
    // Context.project_root), not a cwd walk. Regression for exec ignoring
    // ctx.project_root when loading the outputs: map.
    let home = TempDir::new().unwrap();
    let slug = "acme/secrets";
    let store_path = create_remote_store(&home, slug);

    // Tag a secret in the managed store.
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &store_path.to_string_lossy(),
            "set",
            "prod/stripe-key",
            "sk_live_proj",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    // Project repo: default_store points at the managed store; outputs:
    // defines the pci-prod label.
    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!(
            "default_store: \"{slug}\"\ncodegen:\n  pci-prod:\n    selectors:\n      - tag:pci\n"
        ),
    )
    .unwrap();

    // Run from an UNRELATED cwd with --project=<path>.
    let elsewhere = tempfile::tempdir().unwrap();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(elsewhere.path())
        .args([
            &format!("--project={}", project_dir.path().to_string_lossy()),
            "exec",
            "pci-prod",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE_KEY=sk_live_proj"));
}

#[test]
fn generate_resolves_outputs_via_explicit_project_flag() {
    // hm-x9r: `--project=<path> generate` from an UNRELATED cwd must load the
    // outputs: map from the project root (ctx.project_config()), not a cwd
    // walk — the same regression class fixed for exec (oracle round 5) and
    // migrate (hm-j3s).
    let home = TempDir::new().unwrap();
    let slug = "acme/secrets";
    let store_path = create_remote_store(&home, slug);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &store_path.to_string_lossy(),
            "set",
            "prod/stripe-key",
            "sk_live_gen",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!(
            "default_store: \"{slug}\"\ncodegen:\n  pci-prod:\n    selectors:\n      - tag:pci\n"
        ),
    )
    .unwrap();

    let elsewhere = tempfile::tempdir().unwrap();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(elsewhere.path())
        .args([
            &format!("--project={}", project_dir.path().to_string_lossy()),
            "generate",
            "--output",
            "pci-prod",
            "--stdout",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("sk_live_gen"));
}

#[test]
fn exec_multiple_refs_injects_union() {
    // hm-y36: `himitsu exec <ref> <ref> -- cmd` injects the union of all
    // refs — here an outputs: label plus a concrete path.
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/stripe-key",
            "sk_multi_a",
        ])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/db-pass",
            "pw_multi_b",
        ])
        .assert()
        .success();

    std::fs::write(
        store.path().join("himitsu.yaml"),
        "codegen:\n  app:\n    selectors: []\n    aliases:\n      MY_STRIPE: prod/stripe-key\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "app",
            "prod/db-pass",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("MY_STRIPE=sk_multi_a"))
        .stdout(predicate::str::contains("DB_PASS=pw_multi_b"));
}

#[test]
fn exec_multiple_refs_conflicting_values_error() {
    // hm-y36: the same env var resolving to DIFFERENT values via different
    // refs is a hard error (half-injected environments are forbidden).
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/api-key",
            "value_one",
        ])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "staging/api-key",
            "value_two",
        ])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "prod/api-key",
            "staging/api-key",
            "--",
            "env",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("API_KEY"))
        .stderr(predicate::str::contains("different values"));
}

#[test]
fn exec_multiple_refs_overlapping_same_secret_tolerated() {
    // hm-y36: two refs matching the SAME secret (same env var, same value)
    // are an idempotent union, not a conflict.
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/stripe-key",
            "sk_overlap",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "prod/*",
            "tag:pci",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE_KEY=sk_overlap"));
}

#[test]
fn exec_multiple_refs_each_must_match() {
    // hm-y36: every named ref must match at least one secret — a dud second
    // ref fails the whole invocation rather than silently injecting less.
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args(["--store", &store_flag(&store), "set", "prod/x", "v"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "prod/x",
            "tag:nothing-here",
            "--",
            "env",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("matched no secrets"));
}

#[test]
fn codegen_sops_warns_on_duplicate_keys() {
    // hm-x7z / ADR-0001: codegen's silent last-wins clobber gains generate's
    // warning. Two paths deriving the same env key (API_KEY) trigger it. No
    // status assertion: the warning is emitted before the sops encryption
    // step, which may or may not be available in the test environment.
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args(["--store", &store_flag(&store), "set", "prod/api-key", "a"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "staging/api-key",
            "b",
        ])
        .assert()
        .success();

    std::fs::write(
        store.path().join("himitsu.yaml"),
        "codegen:\n  app:\n    selectors:\n      - prod/*\n      - staging/*\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args(["--store", &store_flag(&store), "codegen", "app"])
        .assert()
        .stderr(predicate::str::contains("duplicate key 'API_KEY'"));
}

#[test]
fn codegen_lang_resolves_outputs_via_explicit_project_flag() {
    // hm-x9r: `--project=<path> codegen --lang ...` from an UNRELATED cwd must
    // load the outputs: map from the project root, not a cwd walk.
    let home = TempDir::new().unwrap();
    let slug = "acme/secrets";
    let store_path = create_remote_store(&home, slug);

    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .args([
            "--store",
            &store_path.to_string_lossy(),
            "set",
            "prod/stripe-key",
            "sk_live_cg",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    init_git_repo(project_dir.path());
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!(
            "default_store: \"{slug}\"\ncodegen:\n  pci-prod:\n    selectors:\n      - tag:pci\n"
        ),
    )
    .unwrap();

    let elsewhere = tempfile::tempdir().unwrap();
    himitsu()
        .env("HIMITSU_CONFIG", home.path().join("config.yaml"))
        .current_dir(elsewhere.path())
        .args([
            &format!("--project={}", project_dir.path().to_string_lossy()),
            "codegen",
            "--lang",
            "typescript",
            "--stdout",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE_KEY"));
}

#[test]
fn exec_output_label_resolves_alias() {
    // An `outputs:` block with an alias must inject under the alias env-var
    // name when the label is exec'd.
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/stripe-key",
            "sk_live_alias",
        ])
        .assert()
        .success();

    let project_cfg = store.path().join("himitsu.yaml");
    std::fs::write(
        &project_cfg,
        "codegen:\n  app:\n    selectors: []\n    aliases:\n      MY_STRIPE: prod/stripe-key\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args(["--store", &store_flag(&store), "exec", "app", "--", "env"])
        .assert()
        .success()
        .stdout(predicate::str::contains("MY_STRIPE=sk_live_alias"));
}

#[test]
fn exec_output_label_resolves_tag_selector_alias() {
    // hm-5i3 / Oracle: an `outputs:` alias whose VALUE is a tag selector
    // (e.g. `STRIPE: tag:stripe`, as documented and produced by migration)
    // must resolve the selector to its single matching secret and inject it
    // under the alias key. Previously this failed because alias values were
    // parsed as concrete refs, so `tag:stripe` errored as an invalid ref.
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/stripe-key",
            "sk_live_tag_alias",
            "--tag",
            "stripe",
        ])
        .assert()
        .success();

    let project_cfg = store.path().join("himitsu.yaml");
    std::fs::write(
        &project_cfg,
        "codegen:\n  app:\n    selectors: []\n    aliases:\n      STRIPE: tag:stripe\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args(["--store", &store_flag(&store), "exec", "app", "--", "env"])
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE=sk_live_tag_alias"));
}

#[test]
fn migrate_envs_preserves_executable_tag_alias() {
    // Roundtrip: a legacy `envs:` block with a tag-selector alias migrates to
    // `outputs:` and the migrated alias is still executable via `exec`.
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/stripe-key",
            "sk_migrated",
            "--tag",
            "stripe",
        ])
        .assert()
        .success();

    // Legacy envs: block with an alias entry whose value is a tag selector.
    let project_cfg = store.path().join("himitsu.yaml");
    std::fs::write(&project_cfg, "envs:\n  app:\n    - STRIPE: tag:stripe\n").unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args(["--store", &store_flag(&store), "migrate", "envs"])
        .assert()
        .success();

    // The migrated config must execute the alias correctly.
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args(["--store", &store_flag(&store), "exec", "app", "--", "env"])
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE=sk_migrated"));
}

#[test]
fn exec_unknown_label_falls_through_to_selector_and_fails() {
    // A ref that is not a defined output AND not a matching selector must
    // still produce the clear "matched no secrets" error (no panic, no hang).
    // Crucially: an `outputs:` block IS configured here but does NOT define
    // the requested label, so this exercises the "outputs configured but
    // unknown label" fall-through (not just the no-outputs path).
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/api-key",
            "sk_prod",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    // outputs: defines `pci-prod`, but we exec a DIFFERENT, undefined label.
    let project_cfg = store.path().join("himitsu.yaml");
    std::fs::write(
        &project_cfg,
        "codegen:\n  pci-prod:\n    selectors:\n      - tag:pci\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "not-a-real-label",
            "--",
            "env",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("matched no secrets"));
}

#[test]
fn exec_cross_store_ref_is_rejected_clearly() {
    // hm-5i3 / Oracle: a documented-but-unsupported cross-store ref
    // (github:org/repo/path) must error with a clear "not supported" message,
    // NOT be silently parsed as a local path and reported as "matched no
    // secrets".
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "github:org/repo/prod/api-key",
            "--",
            "env",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cross-store exec ref"))
        .stderr(predicate::str::contains("matched no secrets").not());
}

#[test]
fn exec_cross_store_output_alias_is_rejected_without_impossible_workaround() {
    // Oracle: an `outputs:` alias that points at a cross-store secret must
    // fail clearly (cross-store exec is not supported) and must NOT suggest
    // the impossible "define a local outputs: alias" workaround.
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/local-key",
            "local_value",
        ])
        .assert()
        .success();

    // outputs: with an alias referencing a cross-store secret.
    let project_cfg = store.path().join("himitsu.yaml");
    std::fs::write(
        &project_cfg,
        "codegen:\n  app:\n    selectors: []\n    aliases:\n      REMOTE_STRIPE: github:acme/secrets#prod/stripe-key\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .current_dir(store.path())
        .args(["--store", &store_flag(&store), "exec", "app", "--", "env"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cross-store"))
        // Must NOT suggest the impossible outputs:-alias workaround.
        .stderr(predicate::str::contains("define a local").not())
        .stderr(predicate::str::contains("codegen:` alias").not());
}

#[test]
fn exec_and_combined_tag_selector_requires_all_tags() {
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    // A: both pci and prod tags
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/stripe-key",
            "sk_live_pci",
            "--tag",
            "pci",
            "--tag",
            "prod",
        ])
        .assert()
        .success();

    // B: only prod tag
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/db-url",
            "postgres_prod_only",
            "--tag",
            "prod",
        ])
        .assert()
        .success();

    // C: only pci tag
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "dev/pci-config",
            "dev_pci_only",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    // exec tag:pci+tag:prod -- env  →  only A (STRIPE_KEY)
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "tag:pci+tag:prod",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE_KEY=sk_live_pci"))
        .stdout(predicate::str::contains("DB_URL=postgres_prod_only").not())
        .stdout(predicate::str::contains("PCI_CONFIG=dev_pci_only").not());
}

#[test]
fn exec_glob_plus_tag_and_combines() {
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    // prod/api-key: pci-tagged, under prod/
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/api-key",
            "prod_pci_value",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    // prod/db-pass: NOT pci-tagged, under prod/
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/db-pass",
            "prod_no_pci",
        ])
        .assert()
        .success();

    // dev/api-key: pci-tagged but NOT under prod/
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "dev/api-key",
            "dev_pci_value",
            "--tag",
            "pci",
        ])
        .assert()
        .success();

    // exec 'prod/*+tag:pci' -- env  →  only prod/api-key → API_KEY
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "prod/*+tag:pci",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("API_KEY=prod_pci_value"))
        .stdout(predicate::str::contains("DB_PASS=prod_no_pci").not())
        .stdout(predicate::str::contains("dev_pci_value").not());
}

#[test]
fn exec_glob_still_works() {
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/api-key",
            "glob_val",
        ])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "dev/secret",
            "dev_val",
        ])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "prod/*",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("API_KEY=glob_val"))
        .stdout(predicate::str::contains("SECRET=dev_val").not());
}

#[test]
fn exec_concrete_path_still_works() {
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/api-key",
            "concrete_val",
        ])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "prod/api-key",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("API_KEY=concrete_val"));
}

#[test]
fn exec_tag_flag_backward_compat_and_filters() {
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    // Two prod/* secrets; only one tagged "rotate"
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/api-key",
            "api_rotate",
            "--tag",
            "rotate",
        ])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "prod/db-pass",
            "db_no_rotate",
        ])
        .assert()
        .success();

    // --tag rotate acts as additional AND filter on top of prod/*
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "prod/*",
            "--tag",
            "rotate",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("API_KEY=api_rotate"))
        .stdout(predicate::str::contains("DB_PASS=db_no_rotate").not());
}

#[test]
fn exec_bare_name_treated_as_concrete_path() {
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "set",
            "pci-prod",
            "bare_path_value",
        ])
        .assert()
        .success();

    // Bare name with no tag: prefix and no * is a concrete path Token::Path
    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "pci-prod",
            "--",
            "env",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PCI_PROD=bare_path_value"));
}

#[test]
fn exec_empty_match_exits_one() {
    let (home, store) = setup();
    let cfg = home.path().join("config.yaml");

    himitsu()
        .env("HIMITSU_CONFIG", &cfg)
        .args([
            "--store",
            &store_flag(&store),
            "exec",
            "tag:nonexistent",
            "--",
            "env",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "error: selector 'tag:nonexistent' matched no secrets",
        ));
}
