//! Store-local persistence for tag definitions.
//!
//! A tag is a named preset stored at `.himitsu/tags/<name>.yaml` inside an
//! explicit store root — deliberately mirroring the one-file-per-secret
//! layout (see [`super::store`]) so tag files diff cleanly and list
//! without decryption. Each file holds a human-readable `description`
//! and a `keys` list of **reference strings** (never secret values):
//! plain store-relative paths, `path:`-qualified paths, or full
//! provider-qualified references for cross-store resolution.
//!
//! This module is pure storage — validation and CRUD/list over an
//! explicit `store: &Path`. No resolution, CLI, TUI, or global-store
//! convention lives here: callers pass an already-resolved store
//! (see Phase 2 of the tag redesign, docs/CODEGEN_DESIGN.md §2.3).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::crypto::tags::validate_tag;
use crate::error::{HimitsuError, Result};
use crate::reference::ReferenceExpr;

// ── Layout & model ──────────────────────────────────────────────────────────

/// Path to the tags directory inside a store.
pub fn tags_dir(store: &Path) -> PathBuf {
    store.join(".himitsu").join("tags")
}

/// Path to a single tag definition file.
fn tag_file_path(store: &Path, name: &str) -> PathBuf {
    tags_dir(store).join(format!("{name}.yaml"))
}

/// A tag definition: `description` plus an ordered list of key
/// references. The on-disk shape matches the redesign doc exactly:
///
/// ```yaml
/// # .himitsu/tags/web-prod.yaml
/// description: Production web service secrets
/// keys:
///   - api-key
///   - personal/stripe-key
///   - github:darkmatter/secrets#api-key
/// ```
///
/// `keys` are **reference strings, never secret values** — zero
/// plaintext at rest. Every reference is validated with
/// [`ReferenceExpr::parse`] on both write and read so a malformed entry
/// fails loudly instead of poisoning later resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagDefinition {
    /// Human-readable description of what the tag groups.
    #[serde(default)]
    pub description: String,
    /// Store-relative paths or full cross-store references.
    #[serde(default)]
    pub keys: Vec<String>,
}

// ── Validation ───────────────────────────────────────────────────────────────

/// Validate a tag name for storage: the shared tag grammar
/// ([`validate_tag`]) plus filesystem-safety rules — `.` and `..` are
/// rejected outright (traversal) and anything that would escape the
/// tags directory is refused.
fn validate_tag_name(name: &str) -> Result<()> {
    validate_tag(name)
        .map(|_| ())
        .map_err(|reason| HimitsuError::InvalidReference(format!("invalid tag name: {reason}")))?;
    if name == "." || name == ".." {
        return Err(HimitsuError::InvalidReference(format!(
            "invalid tag name {name:?}"
        )));
    }
    Ok(())
}

/// Validate one `keys` entry as a reference expression. The entry may
/// stay a bare name — the tag-or-path ambiguity is resolved by Phase 3,
/// not by storage.
fn validate_key(key: &str) -> Result<()> {
    ReferenceExpr::parse(key).map(|_| ()).map_err(|e| {
        HimitsuError::InvalidReference(format!("invalid key reference {key:?} in tag: {e}"))
    })
}

/// Validate a whole definition (name + keys) before any filesystem work.
fn validate_definition(name: &str, def: &TagDefinition) -> Result<()> {
    validate_tag_name(name)?;
    for key in &def.keys {
        validate_key(key)?;
    }
    Ok(())
}

// ── CRUD & list ──────────────────────────────────────────────────────────────

/// Create a new tag definition. Refuses to overwrite an existing tag
/// (no-clobber contract) and creates `.himitsu/tags/` on first use.
///
/// The no-clobber guarantee is enforced by the rename itself
/// ([`NamedTempFile::persist_noclobber`]), not by a separate
/// exists-check — a concurrent creator of the same tag loses the
/// rename race and sees `already exists`, never a silent overwrite.
pub fn create_tag(store: &Path, name: &str, def: &TagDefinition) -> Result<()> {
    validate_definition(name, def)?;
    write_tag_file(&tag_file_path(store, name), def, /* clobber */ false)
}

/// Read a tag definition by name. The name is validated before any
/// filesystem access, so a malformed name can never escape the tags
/// directory. Errors with [`HimitsuError::TagNotFound`] when the tag
/// does not exist.
pub fn read_tag(store: &Path, name: &str) -> Result<TagDefinition> {
    validate_tag_name(name)?;
    let path = tag_file_path(store, name);
    if !path.exists() {
        return Err(HimitsuError::TagNotFound(name.to_string()));
    }
    let def: TagDefinition =
        serde_yaml::from_str(&std::fs::read_to_string(&path)?).map_err(|e| {
            HimitsuError::InvalidConfig(format!("failed to parse tag file {}: {e}", path.display()))
        })?;
    validate_definition(name, &def)?;
    Ok(def)
}

/// Update an existing tag definition in place via an atomic replace.
/// Missing tags are an error.
pub fn update_tag(store: &Path, name: &str, def: &TagDefinition) -> Result<()> {
    validate_definition(name, def)?;
    let path = tag_file_path(store, name);
    if !path.exists() {
        return Err(HimitsuError::TagNotFound(name.to_string()));
    }
    write_tag_file(&path, def, /* clobber */ true)
}

/// Delete a tag definition by name. Missing tags are an error.
pub fn delete_tag(store: &Path, name: &str) -> Result<()> {
    validate_tag_name(name)?;
    let path = tag_file_path(store, name);
    if !path.exists() {
        return Err(HimitsuError::TagNotFound(name.to_string()));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

/// List all tag names in the store, sorted alphabetically. Missing
/// `.himitsu/tags/` directory yields an empty list. Only names that
/// pass the storage grammar are returned; a stray file with a
/// malformed name is skipped, not surfaced.
pub fn list_tags(store: &Path) -> Result<Vec<String>> {
    let dir = tags_dir(store);
    let mut names = vec![];
    if !dir.exists() {
        return Ok(names);
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        let Some(name) = file_name.strip_suffix(".yaml") else {
            continue;
        };
        if validate_tag_name(name).is_err() {
            continue;
        }
        names.push(name.to_string());
    }
    names.sort();
    Ok(names)
}

// ── Write path ────────────────────────────────────────────────────────────────

/// Serialize a tag definition to `path` via a temp file in the same
/// directory, so a crash mid-write never leaves a torn
/// `.himitsu/tags/<name>.yaml`. `clobber = false` creates
/// (fails if the file exists); `clobber = true` replaces atomically.
fn write_tag_file(path: &Path, def: &TagDefinition, clobber: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let yaml = serde_yaml::to_string(def)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, yaml.as_bytes())?;
    if clobber {
        tmp.persist(path).map_err(|err| err.error)?;
    } else {
        tmp.persist_noclobber(path).map_err(|err| {
            if err.error.kind() == std::io::ErrorKind::AlreadyExists {
                HimitsuError::InvalidReference(format!(
                    "tag '{}' already exists",
                    path.file_stem().unwrap_or_default().to_string_lossy()
                ))
            } else {
                err.error.into()
            }
        })?;
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn create_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let def = TagDefinition {
            description: "Production web service secrets".into(),
            keys: vec![
                "api-key".into(),
                "personal/stripe-key".into(),
                "path:prod/DB_PASS".into(),
                "github:darkmatter/secrets#api-key".into(),
                "tag:web-prod".into(),
            ],
        };
        create_tag(tmp.path(), "web-prod", &def).unwrap();

        // Metadata roundtrips exactly, in order.
        assert_eq!(read_tag(tmp.path(), "web-prod").unwrap(), def);
        assert!(tag_file_path(tmp.path(), "web-prod").exists());
    }

    #[test]
    fn create_does_not_clobber_existing_tag() {
        let tmp = TempDir::new().unwrap();
        create_tag(tmp.path(), "pci", &TagDefinition::default()).unwrap();
        let err = create_tag(
            tmp.path(),
            "pci",
            &TagDefinition {
                description: "second".into(),
                keys: vec![],
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"), "was: {err}");
        // Original content untouched.
        assert_eq!(
            read_tag(tmp.path(), "pci").unwrap(),
            TagDefinition::default()
        );
    }

    #[test]
    fn update_rewrites_in_place_and_requires_existing() {
        let tmp = TempDir::new().unwrap();
        create_tag(
            tmp.path(),
            "pci",
            &TagDefinition {
                description: "before".into(),
                keys: vec!["a".into()],
            },
        )
        .unwrap();
        update_tag(
            tmp.path(),
            "pci",
            &TagDefinition {
                description: "after".into(),
                keys: vec!["b".into(), "c".into()],
            },
        )
        .unwrap();
        assert_eq!(
            read_tag(tmp.path(), "pci").unwrap(),
            TagDefinition {
                description: "after".into(),
                keys: vec!["b".into(), "c".into()],
            }
        );

        let err = update_tag(tmp.path(), "missing", &TagDefinition::default()).unwrap_err();
        assert!(matches!(err, HimitsuError::TagNotFound(_)), "was: {err}");
    }

    #[test]
    fn delete_removes_file_and_errors_when_missing() {
        let tmp = TempDir::new().unwrap();
        create_tag(tmp.path(), "pci", &TagDefinition::default()).unwrap();
        delete_tag(tmp.path(), "pci").unwrap();
        assert!(!tag_file_path(tmp.path(), "pci").exists());

        let err = delete_tag(tmp.path(), "pci").unwrap_err();
        assert!(matches!(err, HimitsuError::TagNotFound(_)), "was: {err}");
    }

    #[test]
    fn list_is_sorted_and_empty_when_dir_missing() {
        let tmp = TempDir::new().unwrap();
        // No .himitsu/tags/ yet — empty, not an error.
        assert_eq!(list_tags(tmp.path()).unwrap(), Vec::<String>::new());

        create_tag(tmp.path(), "zeta", &TagDefinition::default()).unwrap();
        create_tag(tmp.path(), "Alpha", &TagDefinition::default()).unwrap();
        create_tag(tmp.path(), "mid", &TagDefinition::default()).unwrap();
        // Non-YAML files, directories, and malformed tag names are ignored.
        std::fs::write(tags_dir(tmp.path()).join("notes.txt"), "x").unwrap();
        std::fs::write(tags_dir(tmp.path()).join("bad name.yaml"), "x").unwrap();
        std::fs::create_dir_all(tags_dir(tmp.path()).join("ops")).unwrap();

        assert_eq!(
            list_tags(tmp.path()).unwrap(),
            vec!["Alpha".to_string(), "mid".to_string(), "zeta".to_string()]
        );
    }

    #[test]
    fn tag_name_validation_rejects_traversal_and_bad_grammar() {
        let tmp = TempDir::new().unwrap();
        for bad in ["", ".", "..", "foo/bar", "foo bar", "a//b", &"a".repeat(65)] {
            let err = create_tag(tmp.path(), bad, &TagDefinition::default()).unwrap_err();
            assert!(matches!(err, HimitsuError::InvalidReference(_)), "{bad:?}");
            assert!(
                !tags_dir(tmp.path()).join(format!("{bad}.yaml")).exists(),
                "{bad:?} must not create a file"
            );
        }
    }

    #[test]
    fn read_and_delete_reject_traversal_names_before_any_fs_access() {
        let tmp = TempDir::new().unwrap();
        // A secret-looking file OUTSIDE the tags dir: a traversal name
        // must be refused by validation, never read or deleted.
        std::fs::write(tmp.path().join("outside.yaml"), "sentinel").unwrap();
        for bad in ["..", "a/../b"] {
            let err = read_tag(tmp.path(), bad).unwrap_err();
            assert!(matches!(err, HimitsuError::InvalidReference(_)), "{bad:?}");
            let err = delete_tag(tmp.path(), bad).unwrap_err();
            assert!(matches!(err, HimitsuError::InvalidReference(_)), "{bad:?}");
        }
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("outside.yaml")).unwrap(),
            "sentinel"
        );
    }

    #[test]
    fn key_validation_rejects_invalid_references() {
        let tmp = TempDir::new().unwrap();
        for bad in [
            "",
            "..",
            "a/../b",
            "path:",
            "tag:",
            "tag:foo bar",
            "github:",
        ] {
            let err = create_tag(
                tmp.path(),
                "web-prod",
                &TagDefinition {
                    description: String::new(),
                    keys: vec![bad.to_string()],
                },
            )
            .unwrap_err();
            assert!(matches!(err, HimitsuError::InvalidReference(_)), "{bad:?}");
        }
        // Nothing was created by the rejected writes.
        assert!(!tags_dir(tmp.path()).join("web-prod.yaml").exists());
    }

    #[test]
    fn read_validates_stored_keys() {
        // A hand-edited tag file with an invalid key fails read loudly.
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tags_dir(tmp.path())).unwrap();
        std::fs::write(
            tags_dir(tmp.path()).join("broken.yaml"),
            "description: x\nkeys:\n  - '..'\n",
        )
        .unwrap();
        let err = read_tag(tmp.path(), "broken").unwrap_err();
        assert!(
            matches!(err, HimitsuError::InvalidReference(_)),
            "was: {err}"
        );
    }

    #[test]
    fn read_missing_tag_is_tag_not_found() {
        let tmp = TempDir::new().unwrap();
        let err = read_tag(tmp.path(), "nope").unwrap_err();
        assert!(matches!(err, HimitsuError::TagNotFound(_)), "was: {err}");
    }

    #[test]
    fn read_malformed_yaml_is_config_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tags_dir(tmp.path())).unwrap();
        std::fs::write(tags_dir(tmp.path()).join("bad.yaml"), "{{invalid").unwrap();
        let err = read_tag(tmp.path(), "bad").unwrap_err();
        assert!(matches!(err, HimitsuError::InvalidConfig(_)), "was: {err}");
    }
}
