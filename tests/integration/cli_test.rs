use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

#[allow(deprecated)]
fn himitsu() -> Command {
    Command::cargo_bin("himitsu").unwrap()
}

/// Set up a user home (keys/config) and a project store root.
///
/// In the new model:
/// - HIMITSU_HOME → data_dir = HIMITSU_HOME/share, state_dir = HIMITSU_HOME/state
/// - `--store <path>` points to the store ROOT (not `.himitsu/` inside it)
/// - Secrets are stored at `store_root/.himitsu/secrets/<path>.age`
/// - Recipients at `store_root/.himitsu/recipients/`
fn setup() -> (TempDir, TempDir) {
    let home = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &store.path().to_string_lossy(), "init"])
        .assert()
        .success();

    (home, store)
}

/// Returns the --store flag value for a given store root TempDir.
fn store_flag(store: &TempDir) -> String {
    store.path().to_string_lossy().to_string()
}

// ============ init tests ============

#[test]
fn init_creates_directory_tree() {
    let (home, store) = setup();
    // Key files at data_dir (HIMITSU_HOME/share/)
    assert!(home.path().join("share/key").exists());
    assert!(home.path().join("share/config.yaml").exists());
    // Store layout at store_root/.himitsu/
    assert!(store.path().join(".himitsu/secrets").exists());
    assert!(store.path().join(".himitsu/recipients/common").exists());
    assert!(store
        .path()
        .join(".himitsu/recipients/common/self.pub")
        .exists());
}

#[test]
fn init_is_idempotent() {
    let (home, store) = setup();
    let key_before = std::fs::read_to_string(home.path().join("share/key")).unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
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
    let self_pub = store.path().join(".himitsu/recipients/common/self.pub");
    assert!(self_pub.exists());
    let contents = std::fs::read_to_string(self_pub).unwrap();
    assert!(contents.starts_with("age1"));
}

// ============ set / get tests ============

#[test]
fn set_get_roundtrip() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/API_KEY", "secret123"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "get", "prod/API_KEY"])
        .assert()
        .success()
        .stdout("secret123");
}

#[test]
fn set_creates_age_file() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/DB_PASS", "hunter2"])
        .assert()
        .success();

    assert!(store
        .path()
        .join(".himitsu/secrets/prod/DB_PASS.age")
        .exists());
}

#[test]
fn set_get_multiline_value() {
    let (home, store) = setup();
    let s = store_flag(&store);
    let multiline = "line1\nline2\nline3";

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/MULTI", multiline])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/SPECIAL", special])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
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
            .env("HIMITSU_HOME", home.path())
            .args(["--store", &s, "set", &format!("{env}/KEY"), "val"])
            .assert()
            .success();
    }

    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/A_KEY", "a"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/B_KEY", "b"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "ls", "prod"])
        .assert()
        .success()
        .stdout(predicate::str::contains("A_KEY"))
        .stdout(predicate::str::contains("B_KEY"));
}

// ============ decrypt test ============

#[test]
fn decrypt_errors_no_plaintext_at_rest() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "decrypt"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not supported"));
}

// ============ encrypt (re-encrypt) test ============

#[test]
fn encrypt_re_encrypts_secrets() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/SECRET", "value"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "encrypt"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Re-encrypted 1 secret"));

    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args([
            "--store",
            &s,
            "recipient",
            "add",
            "mydevice",
            "--self",
            "--group",
            "team",
        ])
        .assert()
        .success();

    let pub_file = store.path().join(".himitsu/recipients/team/mydevice.pub");
    assert!(pub_file.exists());
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
        .env("HIMITSU_HOME", home.path())
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

    assert!(store
        .path()
        .join(".himitsu/recipients/common/bot.pub")
        .exists());
}

#[test]
fn recipient_rm() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "recipient", "add", "todelete", "--self"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "recipient", "rm", "todelete"])
        .assert()
        .success();

    assert!(!store
        .path()
        .join(".himitsu/recipients/common/todelete.pub")
        .exists());
}

#[test]
fn recipient_ls() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "recipient", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("common/self"));
}

// ============ group tests ============

#[test]
fn group_add_creates_directory() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "group", "add", "admins"])
        .assert()
        .success();

    assert!(store.path().join(".himitsu/recipients/admins").exists());
}

#[test]
fn group_rm_removes_directory() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "group", "add", "temp"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "group", "rm", "temp"])
        .assert()
        .success();

    assert!(!store.path().join(".himitsu/recipients/temp").exists());
}

#[test]
fn group_rm_common_rejected() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "group", "rm", "common"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("reserved"));
}

#[test]
fn group_ls_shows_counts() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "group", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("common"))
        .stdout(predicate::str::contains("1 recipient(s)"));
}

// ============ search tests ============

#[test]
fn search_matches_keys() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/STRIPE_KEY", "sk_test"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "search", "STRIPE", "--refresh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE_KEY"));
}

#[test]
fn search_no_matches_returns_empty() {
    let (home, _store) = setup();

    // search without --store: scans stores_dir which is empty
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["search", "NONEXISTENT", "--refresh"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

// ============ version and help tests ============

#[test]
fn version_prints() {
    himitsu()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("himitsu 0.1.0"));
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
        .stdout(predicate::str::contains("group"))
        .stdout(predicate::str::contains("git"));
}

// ============ --remote flag tests ============

/// Create and initialise a remote store at `HIMITSU_HOME/state/stores/<org>/<repo>`.
/// This is where `--remote slug` looks for stores.
fn create_remote_store(home: &TempDir, slug: &str) -> std::path::PathBuf {
    let (org, repo) = slug.split_once('/').unwrap();
    let dest = home.path().join("state/stores").join(org).join(repo);
    std::fs::create_dir_all(&dest).unwrap();
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["--remote", slug, "get", "dev/DB_PASS"])
        .assert()
        .success()
        .stdout("hunter2");
}

#[test]
fn remote_flag_fails_for_unknown_slug() {
    let (home, _store) = setup();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "ghost/missing", "ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ghost/missing"));
}

#[test]
fn remote_flag_rejects_invalid_slug() {
    let (home, _store) = setup();

    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
    git(&["commit", "-m", "init"]);
}

#[test]
fn remote_add_clones_local_repo() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let source = tempfile::tempdir().unwrap();
    create_local_git_repo(source.path());

    himitsu()
        .env("HIMITSU_HOME", home.path())
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

    // The remote should be cloned into stores_dir = HIMITSU_HOME/state/stores/
    assert!(home.path().join("state/stores/test-org/my-repo").exists());
}

#[test]
fn remote_add_resolves_via_remote_flag() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let source = tempfile::tempdir().unwrap();
    create_local_git_repo(source.path());

    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "acme/repo", "ls"])
        .assert()
        .stderr(predicate::str::contains("remote not found").not());
}

#[test]
fn remote_add_duplicate_fails() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let source = tempfile::tempdir().unwrap();
    create_local_git_repo(source.path());
    let url = source.path().to_string_lossy().to_string();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "remote", "add", "dup/repo", "--url", &url])
        .assert()
        .success();

    // Second add with the same slug must fail.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "remote", "add", "dup/repo", "--url", &url])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn remote_add_invalid_slug_fails() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "remote", "add", "notaslug"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid"));
}

// ============ sync tests ============

/// Build a remote data store that already has a secret at `secret_path`.
fn setup_remote_with_secret(
    home: &TempDir,
    slug: &str,
    secret_path: &str,
    value: &str,
) -> std::path::PathBuf {
    let remote_store = create_remote_store(home, slug);
    let rs = remote_store.to_string_lossy().to_string();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &rs, "set", secret_path, value])
        .assert()
        .success();
    remote_store
}

#[test]
fn sync_bind_writes_remote_yaml() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // The remote store directory must exist before binding.
    create_remote_store(&home, "org/proj");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync", "--bind", "org/proj"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Bound store to remote org/proj"));

    // binding file is at store_root/remote.yaml
    let remote_yaml = store.path().join("remote.yaml");
    assert!(remote_yaml.exists(), "remote.yaml should be written");
    let contents = std::fs::read_to_string(&remote_yaml).unwrap();
    assert!(contents.contains("org/proj"));
}

#[test]
fn sync_bind_fails_for_unknown_remote() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync", "--bind", "ghost/missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ghost/missing"));
}

#[test]
fn sync_without_bind_fails_with_useful_error() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no remote binding"));
}

#[test]
fn sync_mirrors_ciphertext_byte_for_byte() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let remote_dir =
        setup_remote_with_secret(&home, "acme/vault", "prod/DB_URL", "postgres://secret");

    // Bind the project store to the remote and then sync.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync", "--bind", "acme/vault"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 secret(s)"));

    // The mirrored .age file must be byte-for-byte identical to the source.
    let remote_age = remote_dir.join(".himitsu/secrets/prod/DB_URL.age");
    let local_age = store.path().join(".himitsu/secrets/prod/DB_URL.age");

    assert!(
        local_age.exists(),
        ".age file should be mirrored to local store"
    );
    assert_eq!(
        std::fs::read(&remote_age).unwrap(),
        std::fs::read(&local_age).unwrap(),
        "mirrored ciphertext must be byte-for-byte identical to source"
    );
}

#[test]
fn sync_mirrors_recipients() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let remote_dir = create_remote_store(&home, "acme/keys");
    let rs = remote_dir.to_string_lossy().to_string();

    // Add an extra recipient to the remote store.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &rs, "recipient", "add", "device2", "--self"])
        .assert()
        .success();

    // Bind and sync.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync", "--bind", "acme/keys"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("recipient file(s)"));

    // device2.pub must appear in the local store after mirroring.
    assert!(
        store
            .path()
            .join(".himitsu/recipients/common/device2.pub")
            .exists(),
        "recipient public-key file must be mirrored"
    );
}

#[test]
fn sync_env_scope_mirrors_only_requested_prefix() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let remote_dir = create_remote_store(&home, "acme/scoped");
    let rs = remote_dir.to_string_lossy().to_string();

    // Populate two path prefixes in the remote.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &rs, "set", "prod/PROD_KEY", "pval"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &rs, "set", "staging/STAGING_KEY", "sval"])
        .assert()
        .success();

    // Bind and sync ONLY the "prod" prefix.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync", "--bind", "acme/scoped"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync", "prod"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 secret(s)"));

    // prod must be present; staging must be absent.
    assert!(
        store
            .path()
            .join(".himitsu/secrets/prod/PROD_KEY.age")
            .exists(),
        "prod secret should be mirrored"
    );
    assert!(
        !store
            .path()
            .join(".himitsu/secrets/staging/STAGING_KEY.age")
            .exists(),
        "staging secret must NOT be mirrored when only 'prod' was requested"
    );
}

#[test]
fn sync_all_when_no_prefix_specified() {
    let (home, store) = setup();
    let s = store_flag(&store);

    let remote_dir = create_remote_store(&home, "acme/allenvs");
    let rs = remote_dir.to_string_lossy().to_string();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &rs, "set", "prod/PK", "pv"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &rs, "set", "staging/SK", "sv"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync", "--bind", "acme/allenvs"])
        .assert()
        .success();

    // Sync without specifying a prefix — should mirror both.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 secret(s)"));

    assert!(store.path().join(".himitsu/secrets/prod/PK.age").exists());
    assert!(store
        .path()
        .join(".himitsu/secrets/staging/SK.age")
        .exists());
}

// ============ git tests ============

#[test]
fn git_help_shows_usage() {
    himitsu()
        .args(["git", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Run git commands inside the himitsu data directory",
        ));
}

#[test]
fn git_init_creates_repo() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // Pipe "y" to accept the git-init prompt
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "init"])
        .write_stdin("y\n")
        .assert()
        .success();

    // data_dir (HIMITSU_HOME/share) gets the .git
    assert!(home.path().join("share/.git").exists());
}

#[test]
fn git_status_after_init() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "init"])
        .write_stdin("y\n")
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "status"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Untracked files")
                .or(predicate::str::contains("nothing to commit")),
        );
}

#[test]
fn git_add_and_commit() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // git init
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "init"])
        .write_stdin("y\n")
        .assert()
        .success();

    // git add -A
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "add", "-A"])
        .assert()
        .success();

    // git commit
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "commit", "-m", "test commit"])
        .assert()
        .success();

    // git log should show our commit
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "log", "--oneline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("test commit"));
}

#[test]
fn git_bare_defaults_to_status() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "init"])
        .write_stdin("y\n")
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git"])
        .assert()
        .success()
        .stdout(predicate::str::contains("branch").or(predicate::str::contains("Untracked")));
}

#[test]
fn git_decline_init_exits_cleanly() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "status"])
        .write_stdin("n\n")
        .assert()
        .success();

    // .git should NOT exist inside share/
    assert!(!home.path().join("share/.git").exists());
}

#[test]
fn git_passthrough_respects_flags() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "init"])
        .write_stdin("y\n")
        .assert()
        .success();

    // `git --no-pager log` in an empty repo should fail (no commits)
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "--no-pager", "log"])
        .assert()
        .failure();
}

#[test]
fn smart_init_prompt_on_missing_home() {
    let home = TempDir::new().unwrap();
    let fake_home = home.path().join("nonexistent");
    let cwd = TempDir::new().unwrap();

    // Running any command with a missing home should prompt, not crash.
    // Decline with "n" — should exit cleanly.
    himitsu()
        .env("HIMITSU_HOME", &fake_home)
        .current_dir(cwd.path())
        .args(["ls"])
        .write_stdin("n\n")
        .assert()
        .success();
}

#[test]
fn smart_init_prompt_accepts_and_continues() {
    let home = TempDir::new().unwrap();
    let fake_home = home.path().join("fresh");
    let cwd = TempDir::new().unwrap();

    // Accept the prompt with "y" — should init then run `ls`
    himitsu()
        .env("HIMITSU_HOME", &fake_home)
        .current_dir(cwd.path())
        .args(["ls"])
        .write_stdin("y\n")
        .assert()
        .success();

    // Verify init actually happened — key file is at data_dir/key
    assert!(fake_home.join("share/key").exists());
}
