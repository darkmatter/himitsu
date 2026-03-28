use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

#[allow(deprecated)]
fn himitsu() -> Command {
    Command::cargo_bin("himitsu").unwrap()
}

/// Set up a user home (keys/config) and a project store (.himitsu/).
fn setup() -> (TempDir, TempDir) {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args([
            "--store",
            &project.path().join(".himitsu").to_string_lossy(),
            "init",
        ])
        .assert()
        .success();

    (home, project)
}

fn store_flag(project: &TempDir) -> String {
    project
        .path()
        .join(".himitsu")
        .to_string_lossy()
        .to_string()
}

// ============ init tests ============

#[test]
fn init_creates_directory_tree() {
    let (home, project) = setup();
    let store = project.path().join(".himitsu");
    assert!(home.path().join("keys/age.txt").exists());
    assert!(home.path().join(".himitsu.yaml").exists());
    assert!(store.join("vars").exists());
    assert!(store.join("recipients/common").exists());
    assert!(store.join("recipients/common/self.pub").exists());
    assert!(store.join("data.json").exists());
}

#[test]
fn init_is_idempotent() {
    let (home, project) = setup();
    let key_before = std::fs::read_to_string(home.path().join("keys/age.txt")).unwrap();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &store_flag(&project), "init"])
        .assert()
        .success();

    let key_after = std::fs::read_to_string(home.path().join("keys/age.txt")).unwrap();
    assert_eq!(key_before, key_after);
}

#[test]
fn init_generates_valid_age_key() {
    let (home, _project) = setup();
    let contents = std::fs::read_to_string(home.path().join("keys/age.txt")).unwrap();
    assert!(contents.contains("AGE-SECRET-KEY-"));
    assert!(contents.contains("# public key: age1"));
}

#[test]
fn init_adds_self_as_recipient() {
    let (_home, project) = setup();
    let self_pub = project.path().join(".himitsu/recipients/common/self.pub");
    assert!(self_pub.exists());
    let contents = std::fs::read_to_string(self_pub).unwrap();
    assert!(contents.starts_with("age1"));
}

// ============ set / get tests ============

#[test]
fn set_get_roundtrip() {
    let (home, project) = setup();
    let s = store_flag(&project);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod", "API_KEY", "secret123"])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "get", "prod", "API_KEY"])
        .assert()
        .success()
        .stdout("secret123");
}

#[test]
fn set_creates_age_file() {
    let (home, project) = setup();
    let s = store_flag(&project);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod", "DB_PASS", "hunter2"])
        .assert()
        .success();

    assert!(project
        .path()
        .join(".himitsu/vars/prod/DB_PASS.age")
        .exists());
}

#[test]
fn set_get_multiline_value() {
    let (home, project) = setup();
    let s = store_flag(&project);
    let multiline = "line1\nline2\nline3";

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod", "MULTI", multiline])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "get", "prod", "MULTI"])
        .assert()
        .success()
        .stdout(multiline);
}

#[test]
fn set_get_special_characters() {
    let (home, project) = setup();
    let s = store_flag(&project);
    let special = r#"hello "world" \n back\slash"#;

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod", "SPECIAL", special])
        .assert()
        .success();

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "get", "prod", "SPECIAL"])
        .assert()
        .success()
        .stdout(special);
}

// ============ ls tests ============

#[test]
fn ls_lists_envs() {
    let (home, project) = setup();
    let s = store_flag(&project);

    for env in &["dev", "prod"] {
        himitsu()
            .env("HIMITSU_HOME", home.path())
            .args(["--store", &s, "set", env, "KEY", "val"])
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
fn ls_lists_keys_in_env() {
    let (home, project) = setup();
    let s = store_flag(&project);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod", "A_KEY", "a"])
        .assert()
        .success();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod", "B_KEY", "b"])
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
    let (home, project) = setup();
    let s = store_flag(&project);

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
    let (home, project) = setup();
    let s = store_flag(&project);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod", "SECRET", "value"])
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
        .args(["--store", &s, "get", "prod", "SECRET"])
        .assert()
        .success()
        .stdout("value");
}

// ============ recipient tests ============

#[test]
fn recipient_add_self() {
    let (home, project) = setup();
    let s = store_flag(&project);

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

    let pub_file = project.path().join(".himitsu/recipients/team/mydevice.pub");
    assert!(pub_file.exists());
}

#[test]
fn recipient_add_explicit_key() {
    let (home, project) = setup();
    let s = store_flag(&project);

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

    assert!(project
        .path()
        .join(".himitsu/recipients/common/bot.pub")
        .exists());
}

#[test]
fn recipient_rm() {
    let (home, project) = setup();
    let s = store_flag(&project);

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

    assert!(!project
        .path()
        .join(".himitsu/recipients/common/todelete.pub")
        .exists());
}

#[test]
fn recipient_ls() {
    let (home, project) = setup();
    let s = store_flag(&project);

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
    let (home, project) = setup();
    let s = store_flag(&project);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "group", "add", "admins"])
        .assert()
        .success();

    assert!(project.path().join(".himitsu/recipients/admins").exists());
}

#[test]
fn group_rm_removes_directory() {
    let (home, project) = setup();
    let s = store_flag(&project);

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

    assert!(!project.path().join(".himitsu/recipients/temp").exists());
}

#[test]
fn group_rm_common_rejected() {
    let (home, project) = setup();
    let s = store_flag(&project);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "group", "rm", "common"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("reserved"));
}

#[test]
fn group_ls_shows_counts() {
    let (home, project) = setup();
    let s = store_flag(&project);

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
    let (home, project) = setup();
    let s = store_flag(&project);

    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "set", "prod", "STRIPE_KEY", "sk_test"])
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
    let (home, _project) = setup();

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

// ============ git tests ============

#[test]
fn git_help_shows_usage() {
    himitsu()
        .args(["git", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Run git commands inside the himitsu directory",
        ));
}

#[test]
fn git_init_creates_repo() {
    let (home, project) = setup();
    let s = store_flag(&project);

    // Pipe "y" to accept the git-init prompt
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "init"])
        .write_stdin("y\n")
        .assert()
        .success();

    assert!(home.path().join(".git").exists());
}

#[test]
fn git_status_after_init() {
    let (home, project) = setup();
    let s = store_flag(&project);

    // First git-init the home dir
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "init"])
        .write_stdin("y\n")
        .assert()
        .success();

    // Now `git status` should work
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
    let (home, project) = setup();
    let s = store_flag(&project);

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
    let (home, project) = setup();
    let s = store_flag(&project);

    // git init first
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "init"])
        .write_stdin("y\n")
        .assert()
        .success();

    // Bare `himitsu git` (no args) should behave like `git status`
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git"])
        .assert()
        .success()
        .stdout(predicate::str::contains("branch").or(predicate::str::contains("Untracked")));
}

#[test]
fn git_decline_init_exits_cleanly() {
    let (home, project) = setup();
    let s = store_flag(&project);

    // Decline the git-init prompt with "n"
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "status"])
        .write_stdin("n\n")
        .assert()
        .success();

    // .git should NOT exist
    assert!(!home.path().join(".git").exists());
}

#[test]
fn git_passthrough_respects_flags() {
    let (home, project) = setup();
    let s = store_flag(&project);

    // git init
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &s, "git", "init"])
        .write_stdin("y\n")
        .assert()
        .success();

    // `git --no-pager log` in an empty repo should fail (no commits)
    // This verifies that flags with hyphens are forwarded correctly.
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
    // Use a bare temp dir as CWD so store discovery doesn't find the
    // project repo's .himitsu/ directory.
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
    // Use a bare temp dir as CWD so store discovery doesn't find the
    // project repo's .himitsu/ directory.
    let cwd = TempDir::new().unwrap();

    // Accept the prompt with "y" — should init then run `ls`
    himitsu()
        .env("HIMITSU_HOME", &fake_home)
        .current_dir(cwd.path())
        .args(["ls"])
        .write_stdin("y\n")
        .assert()
        .success();

    // Verify init actually happened
    assert!(fake_home.join("keys/age.txt").exists());
}
