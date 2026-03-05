use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

#[allow(deprecated)]
fn himitsu() -> Command {
    Command::cargo_bin("himitsu").unwrap()
}

/// Create a fake HIMITSU_HOME and run init.
fn setup_home() -> TempDir {
    let home = TempDir::new().unwrap();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .arg("init")
        .assert()
        .success();
    home
}

/// Set up a local remote by creating the directory structure manually.
fn setup_remote(home: &TempDir, remote_ref: &str) {
    let remote_path = home.path().join("data").join(remote_ref);
    std::fs::create_dir_all(remote_path.join("vars")).unwrap();
    std::fs::create_dir_all(remote_path.join("recipients/common")).unwrap();
    std::fs::write(remote_path.join("data.json"), "{\"groups\":[\"common\"]}").unwrap();

    // Copy the user's public key as a recipient
    let age_txt = std::fs::read_to_string(home.path().join("keys/age.txt")).unwrap();
    let pubkey = age_txt
        .lines()
        .find(|l| l.starts_with("# public key: "))
        .unwrap()
        .strip_prefix("# public key: ")
        .unwrap();
    std::fs::write(
        remote_path.join("recipients/common/self.pub"),
        format!("{pubkey}\n"),
    )
    .unwrap();
}

// ============ init tests ============

#[test]
fn init_creates_directory_tree() {
    let home = setup_home();
    assert!(home.path().join("keys/age.txt").exists());
    assert!(home.path().join("config.yaml").exists());
    assert!(home.path().join("state").exists());
    assert!(home.path().join("data").exists());
    assert!(home.path().join("cache").exists());
    assert!(home.path().join("locks").exists());
}

#[test]
fn init_is_idempotent() {
    let home = setup_home();
    let key_before = std::fs::read_to_string(home.path().join("keys/age.txt")).unwrap();

    // Run init again
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .arg("init")
        .assert()
        .success();

    // Key should not change
    let key_after = std::fs::read_to_string(home.path().join("keys/age.txt")).unwrap();
    assert_eq!(key_before, key_after);
}

#[test]
fn init_generates_valid_age_key() {
    let home = setup_home();
    let contents = std::fs::read_to_string(home.path().join("keys/age.txt")).unwrap();
    assert!(contents.contains("AGE-SECRET-KEY-"));
    assert!(contents.contains("# public key: age1"));
}

// ============ set / get tests ============

#[test]
fn set_get_roundtrip() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "set", "prod", "API_KEY", "secret123"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "get", "prod", "API_KEY"])
        .assert()
        .success()
        .stdout("secret123");
}

#[test]
fn set_creates_age_file() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "set", "prod", "DB_PASS", "hunter2"])
        .assert()
        .success();

    let age_file = home.path().join("data/test/repo/vars/prod/DB_PASS.age");
    assert!(age_file.exists());
}

#[test]
fn set_get_multiline_value() {
    let home = setup_home();
    setup_remote(&home, "test/repo");
    let multiline = "line1\nline2\nline3";

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "set", "prod", "MULTI", multiline])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "get", "prod", "MULTI"])
        .assert()
        .success()
        .stdout(multiline);
}

#[test]
fn set_get_special_characters() {
    let home = setup_home();
    setup_remote(&home, "test/repo");
    let special = r#"hello "world" \n 🎉 back\slash"#;

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "set", "prod", "SPECIAL", special])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "get", "prod", "SPECIAL"])
        .assert()
        .success()
        .stdout(special);
}

// ============ ls tests ============

#[test]
fn ls_lists_envs() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    // Create secrets in two envs
    for env in &["dev", "prod"] {
        himitsu()
            .env("HIMITSU_HOME", home.path())
            .args(["-r", "test/repo", "set", env, "KEY", "val"])
            .assert()
            .success();
    }

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dev"))
        .stdout(predicate::str::contains("prod"));
}

#[test]
fn ls_lists_keys_in_env() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "set", "prod", "A_KEY", "a"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "set", "prod", "B_KEY", "b"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "ls", "prod"])
        .assert()
        .success()
        .stdout(predicate::str::contains("A_KEY"))
        .stdout(predicate::str::contains("B_KEY"));
}

// ============ decrypt test ============

#[test]
fn decrypt_errors_no_plaintext_at_rest() {
    let home = setup_home();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["decrypt"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not supported"));
}

// ============ encrypt (re-encrypt) test ============

#[test]
fn encrypt_re_encrypts_secrets() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "set", "prod", "SECRET", "value"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "encrypt"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Re-encrypted 1 secret"));

    // Verify we can still read the secret after re-encryption
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "get", "prod", "SECRET"])
        .assert()
        .success()
        .stdout("value");
}

// ============ recipient tests ============

#[test]
fn recipient_add_self() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args([
            "-r",
            "test/repo",
            "recipient",
            "add",
            "mydevice",
            "--self",
            "--group",
            "team",
        ])
        .assert()
        .success();

    let pub_file = home
        .path()
        .join("data/test/repo/recipients/team/mydevice.pub");
    assert!(pub_file.exists());
    let contents = std::fs::read_to_string(pub_file).unwrap();
    assert!(contents.starts_with("age1"));
}

#[test]
fn recipient_add_explicit_key() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    // Generate a key to use
    let age_txt = std::fs::read_to_string(home.path().join("keys/age.txt")).unwrap();
    let pubkey = age_txt
        .lines()
        .find(|l| l.starts_with("# public key: "))
        .unwrap()
        .strip_prefix("# public key: ")
        .unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args([
            "-r",
            "test/repo",
            "recipient",
            "add",
            "bot",
            "--age-key",
            pubkey,
            "--group",
            "common",
        ])
        .assert()
        .success();

    let pub_file = home.path().join("data/test/repo/recipients/common/bot.pub");
    assert!(pub_file.exists());
}

#[test]
fn recipient_rm() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args([
            "-r",
            "test/repo",
            "recipient",
            "add",
            "todelete",
            "--self",
            "--group",
            "common",
        ])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args([
            "-r",
            "test/repo",
            "recipient",
            "rm",
            "todelete",
            "--group",
            "common",
        ])
        .assert()
        .success();

    let pub_file = home
        .path()
        .join("data/test/repo/recipients/common/todelete.pub");
    assert!(!pub_file.exists());
}

#[test]
fn recipient_ls() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "recipient", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("common/self"));
}

// ============ group tests ============

#[test]
fn group_add_creates_directory() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "group", "add", "admins"])
        .assert()
        .success();

    let group_dir = home.path().join("data/test/repo/recipients/admins");
    assert!(group_dir.exists());
}

#[test]
fn group_rm_removes_directory() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "group", "add", "temp"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "group", "rm", "temp"])
        .assert()
        .success();

    let group_dir = home.path().join("data/test/repo/recipients/temp");
    assert!(!group_dir.exists());
}

#[test]
fn group_rm_common_rejected() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "group", "rm", "common"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("reserved"));
}

#[test]
fn group_ls_shows_counts() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "group", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("common"))
        .stdout(predicate::str::contains("1 recipient(s)"));
}

// ============ search tests ============

#[test]
fn search_matches_keys() {
    let home = setup_home();
    setup_remote(&home, "test/repo");

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["-r", "test/repo", "set", "prod", "STRIPE_KEY", "sk_test"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["search", "STRIPE", "--refresh"])
        .env("HIMITSU_HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("STRIPE_KEY"));
}

#[test]
fn search_no_matches_returns_empty() {
    let home = setup_home();

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
        .stdout(predicate::str::contains("remote"));
}
