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

// ============ rekey (re-encrypt) tests ============

#[test]
fn rekey_re_encrypts_secrets() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/SECRET", "value"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "rekey"])
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

#[test]
fn encrypt_deprecated_wrapper_still_works() {
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
        .stdout(predicate::str::contains("Re-encrypted"))
        .stderr(predicate::str::contains("deprecated"));
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
fn version_prints_with_commit_sha() {
    himitsu()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("himitsu 0.1.0 (commit "))
        .stdout(predicate::str::is_match(r"(?:[0-9a-f]{40}|unknown)").unwrap());
}

#[test]
fn version_subcommand_prints_without_initializing() {
    let home = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains("himitsu 0.1.0 (commit "))
        .stdout(predicate::str::is_match(r"(?:[0-9a-f]{40}|unknown)").unwrap());

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
        .stdout(predicate::str::contains("group"))
        .stdout(predicate::str::contains("rekey"))
        .stdout(predicate::str::contains("sync"))
        .stdout(predicate::str::contains("git"))
        .stdout(predicate::str::contains("generate"))
        .stdout(predicate::str::contains("remote"))
        .stdout(predicate::str::contains("ls"))
        .stdout(predicate::str::contains("version"));
}

#[test]
fn help_does_not_show_hidden_commands() {
    // Hidden commands should not appear in the default --help output.
    // 'encrypt' is deprecated+hidden; 'codegen', 'schema' are demoted to hidden;
    // 'share', 'inbox', 'import' are not yet implemented.
    himitsu()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("codegen").not())
        .stdout(predicate::str::contains("schema").not())
        .stdout(predicate::str::contains("  encrypt").not()) // hidden deprecated alias
        .stdout(predicate::str::contains("  share").not())
        .stdout(predicate::str::contains("  inbox").not())
        .stdout(predicate::str::contains("  import").not())
        .stdout(predicate::str::contains("  decrypt").not());
}

#[test]
fn recipient_show_existing() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // Add a self recipient so there's something to find.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "recipient", "add", "mykey", "--self"])
        .assert()
        .success();

    // show should print the public key
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "recipient", "show", "mykey"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("age1"));
}

#[test]
fn recipient_show_nonexistent() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "recipient", "show", "nobody"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
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

// ============ remote default/list/remove tests ============

#[test]
fn remote_list_shows_all() {
    let (home, store) = setup();
    let s = store_flag(&store);
    create_remote_store(&home, "acme/secrets");
    create_remote_store(&home, "myorg/keys");
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "remote", "default", "acme/secrets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Default store set to"));
    // Read it back — should echo the slug.
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "remote", "default", "acme/vault"])
        .assert()
        .success();
    // Remove the store.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "remote", "remove", "acme/vault"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Cleared default store"));
    // Default should now be "none set".
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync", "acme/vault"])
        .assert()
        .success()
        .stdout(predicate::str::contains("acme/vault"));
}

#[test]
fn sync_no_rekey_flag_skips_rekey() {
    // With --no-rekey, output says "pulled" and does NOT say "rekeyed".
    let (home, store) = setup();
    let s = store_flag(&store);
    create_remote_store(&home, "acme/vault");
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "sync", "acme/vault", "--no-rekey"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pulled"))
        .stdout(predicate::str::contains("rekeyed").not());
}

#[test]
fn sync_invalid_slug_fails() {
    // A slug without an org/repo separator is rejected.
    let (home, store) = setup();
    let s = store_flag(&store);
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", &fake_home)
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
        .env("HIMITSU_HOME", &fake_home)
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
        .env("HIMITSU_HOME", &fake_home)
        .current_dir(cwd.path())
        .args(["ls"])
        .assert()
        .success();
}

// ============ init wizard output tests ============

#[test]
fn init_shows_wizard_output_on_first_run() {
    let home = TempDir::new().unwrap();

    // First-ever init: wizard summary.
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["init", "--name", slug])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ Registered store myorg/myproject (default)",
        ));

    // The store directory should exist under stores_dir.
    assert!(home.path().join("state/stores/myorg/myproject").exists());

    // The global config should have default_store set.
    let cfg_text = std::fs::read_to_string(home.path().join("share/config.yaml")).unwrap();
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["remote", "default", slug])
        .assert()
        .success();

    // Now `ls` should succeed without any --remote or --store flag.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["ls"])
        .assert()
        .success();
}

#[test]
fn resolve_store_via_project_config() {
    // Place a himitsu.yaml with default_store in a temp project directory.
    let (home, _store) = setup();
    let slug = "myorg/projected";
    create_remote_store(&home, slug);

    let project_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!("default_store: \"{slug}\"\n"),
    )
    .unwrap();

    // Running `ls` from the project directory should resolve via the project config.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .current_dir(project_dir.path())
        .args(["ls"])
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
        .env("HIMITSU_HOME", home.path())
        .current_dir(project_dir.path())
        .args(["-r", override_slug, "ls"])
        .assert()
        .success();
}

#[test]
fn resolve_store_project_config_over_global_config() {
    // A project-level himitsu.yaml default_store takes precedence over the
    // global config default_store (but --remote still wins).
    let (home, _store) = setup();
    let global_slug = "myorg/global";
    let project_slug = "myorg/local";
    create_remote_store(&home, global_slug);
    create_remote_store(&home, project_slug);

    // Set global default.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["remote", "default", global_slug])
        .assert()
        .success();

    // Write a secret to the project store so we can tell which one was used.
    let project_path = home.path().join("state/stores/myorg/local");
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
    std::fs::write(
        project_dir.path().join("himitsu.yaml"),
        format!("default_store: \"{project_slug}\"\n"),
    )
    .unwrap();

    // Should read the secret from the project store, not the global one.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .current_dir(project_dir.path())
        .args(["get", "prod/MARKER"])
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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

    // Set secrets in the store.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "dev/DB_PASSWORD", "hunter2"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "dev/API_KEY", "sk_dev_123"])
        .assert()
        .success();

    // Create a project config in the store directory (or any ancestor).
    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(
        project_dir.path(),
        "envs:\n  dev:\n    - dev/DB_PASSWORD\n    - dev/API_KEY\n",
    );

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "generate", "--stdout", "--env", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("DB_PASSWORD"))
        .stdout(predicate::str::contains("hunter2"))
        .stdout(predicate::str::contains("API_KEY"))
        .stdout(predicate::str::contains("sk_dev_123"));
}

#[test]
fn generate_alias_entry() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "dev/DB_PASSWORD", "secretpass"])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    // Alias: output key MY_DB_PASS, value from dev/DB_PASSWORD
    write_project_config(
        project_dir.path(),
        "envs:\n  dev:\n    - MY_DB_PASS: dev/DB_PASSWORD\n",
    );

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "generate", "--stdout", "--env", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("MY_DB_PASS"))
        .stdout(predicate::str::contains("secretpass"));
}

#[test]
fn generate_glob_entry() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // Set multiple secrets under dev/
    for (path, val) in [("dev/ALPHA", "alpha123"), ("dev/BETA", "beta456")] {
        himitsu()
            .env("HIMITSU_HOME", home.path())
            .args(["--store", &s, "set", path, val])
            .assert()
            .success();
    }

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(project_dir.path(), "envs:\n  dev:\n    - \"dev/*\"\n");

    let out = himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "generate", "--stdout", "--env", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .success();

    out.stdout(predicate::str::contains("ALPHA"))
        .stdout(predicate::str::contains("alpha123"))
        .stdout(predicate::str::contains("BETA"))
        .stdout(predicate::str::contains("beta456"));
}

#[test]
fn generate_single_entry_only_that_key() {
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "dev/API_KEY", "only_this"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "dev/OTHER", "not_this"])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    // Config references only dev/API_KEY — OTHER must not appear.
    write_project_config(project_dir.path(), "envs:\n  dev:\n    - dev/API_KEY\n");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "generate", "--stdout", "--env", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("API_KEY"))
        .stdout(predicate::str::contains("only_this"))
        .stdout(predicate::str::is_match("(?m)^").unwrap()) // sanity
        .stdout(predicate::str::contains("OTHER").not())
        .stdout(predicate::str::contains("not_this").not());
}

#[test]
fn generate_no_project_config_errors() {
    let (home, store) = setup();
    let s = store_flag(&store);

    // Run from a directory that has no himitsu.yaml anywhere above it.
    let empty_dir = tempfile::tempdir().unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
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
    write_project_config(project_dir.path(), "envs:\n  dev:\n    - dev/X\n");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args([
            "--store",
            &s,
            "generate",
            "--stdout",
            "--env",
            "nonexistent",
        ])
        .current_dir(project_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("nonexistent"));
}

#[test]
fn project_config_discovers_dotconfig_variant() {
    // .config/himitsu.yaml should also be discovered.
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "dev/KEY", "dotcfg_val"])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    let config_subdir = project_dir.path().join(".config");
    std::fs::create_dir_all(&config_subdir).unwrap();
    std::fs::write(
        config_subdir.join("himitsu.yaml"),
        "envs:\n  dev:\n    - dev/KEY\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "generate", "--stdout", "--env", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("KEY"))
        .stdout(predicate::str::contains("dotcfg_val"));
}

#[test]
fn project_config_discovers_dothimitsu_variant() {
    // .himitsu/config.yaml at the project root should also be discovered.
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "dev/SECRET", "dothimitsu_val"])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    let himitsu_subdir = project_dir.path().join(".himitsu");
    std::fs::create_dir_all(&himitsu_subdir).unwrap();
    std::fs::write(
        himitsu_subdir.join("config.yaml"),
        "envs:\n  dev:\n    - dev/SECRET\n",
    )
    .unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "generate", "--stdout", "--env", "dev"])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("SECRET"))
        .stdout(predicate::str::contains("dothimitsu_val"));
}

#[test]
fn generate_all_envs_stdout() {
    // When --env is omitted, all envs are generated (interleaved on stdout).
    let (home, store) = setup();
    let s = store_flag(&store);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "dev/D_KEY", "devval"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/P_KEY", "prodval"])
        .assert()
        .success();

    let project_dir = tempfile::tempdir().unwrap();
    write_project_config(
        project_dir.path(),
        "envs:\n  dev:\n    - dev/D_KEY\n  prod:\n    - prod/P_KEY\n",
    );

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "generate", "--stdout"])
        .current_dir(project_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("D_KEY"))
        .stdout(predicate::str::contains("devval"))
        .stdout(predicate::str::contains("P_KEY"))
        .stdout(predicate::str::contains("prodval"));
}

// ============ provider-prefixed path tests ============

/// Set up a named store at `stores_dir/org/repo` using `himitsu init --name`.
/// Returns the home TempDir with keys and config.
fn setup_named_store(home: &TempDir, slug: &str) {
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["set", "github:acme/secrets/staging/DB_PASS", "dbpass456"])
        .assert()
        .success();

    // Verify by reading back with --remote.
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["--remote", "acme/secrets", "set", "prod/API_KEY", "v1"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--remote", "acme/secrets", "set", "dev/API_KEY", "v2"])
        .assert()
        .success();

    // `ls github:acme/secrets` → list all secrets in that store.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["ls", "github:acme/secrets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prod/API_KEY"))
        .stdout(predicate::str::contains("dev/API_KEY"));
}

#[test]
fn ls_with_qualified_prefix_filters_secrets() {
    let home = TempDir::new().unwrap();

    setup_named_store(&home, "acme/secrets");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--remote", "acme/secrets", "set", "prod/API_KEY", "v1"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--remote", "acme/secrets", "set", "dev/API_KEY", "v2"])
        .assert()
        .success();

    // `ls github:acme/secrets/prod` → only prod/* secrets.
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["ls", "github:acme/secrets/prod"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prod/API_KEY"))
        .stdout(predicate::str::contains("dev/API_KEY").not());
}

#[test]
fn get_qualified_unknown_store_returns_error() {
    let home = TempDir::new().unwrap();
    // Init home (keys only, no named store).
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["init"])
        .assert()
        .success();

    // Requesting a store that doesn't exist should error.
    // We need at least one store registered so resolve_store doesn't fail
    // before we reach the command handler. Register a dummy store.
    setup_named_store(&home, "dummy/store");

    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod/TOKEN", "value123"])
        .assert()
        .success();

    // `get` should decrypt the secret successfully.
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "rekey"])
        .assert()
        .success();

    // `get` should still decrypt correctly after rekey.
    himitsu()
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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

    // The new key file should be under the custom path.
    assert!(store
        .path()
        .join("my/recipients/common/extra-key.pub")
        .exists());
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
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
        .env("HIMITSU_HOME", home.path())
        .args(["check", "myorg/offline", "--offline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("myorg/offline"));
}
