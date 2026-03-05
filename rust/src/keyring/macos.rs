use std::process::Command;

use crate::error::{HimitsuError, Result};
use crate::keyring::{KeyProvider, KEY_SERVICE, SCOPE_SERVICE};

/// macOS Keychain adapter using the `security` CLI.
pub struct MacOSKeychain;

impl MacOSKeychain {
    /// Check if we're running on macOS.
    pub fn is_available() -> bool {
        cfg!(target_os = "macos")
    }

    fn security_add(service: &str, account: &str, password: &str) -> Result<()> {
        let output = Command::new("security")
            .args([
                "add-generic-password",
                "-s",
                service,
                "-a",
                account,
                "-w",
                password,
                "-U", // update if exists
            ])
            .output()
            .map_err(|e| HimitsuError::Keychain(format!("failed to run security: {e}")))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(HimitsuError::Keychain(format!(
                "security add-generic-password failed: {}",
                stderr.trim()
            )))
        }
    }

    fn security_find(service: &str, account: &str) -> Result<Option<String>> {
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                service,
                "-a",
                account,
                "-w", // print password only
            ])
            .output()
            .map_err(|e| HimitsuError::Keychain(format!("failed to run security: {e}")))?;

        if output.status.success() {
            let password = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(Some(password))
        } else {
            // Item not found is not an error, just means it doesn't exist
            Ok(None)
        }
    }
}

impl KeyProvider for MacOSKeychain {
    fn store_key(&self, fingerprint: &str, secret_key: &str) -> Result<()> {
        Self::security_add(KEY_SERVICE, fingerprint, secret_key)
    }

    fn load_key(&self, fingerprint: &str) -> Result<Option<String>> {
        Self::security_find(KEY_SERVICE, fingerprint)
    }

    fn store_scope(&self, scope: &str, fingerprint: &str) -> Result<()> {
        Self::security_add(SCOPE_SERVICE, scope, fingerprint)
    }

    fn load_scope(&self, scope: &str) -> Result<Option<String>> {
        Self::security_find(SCOPE_SERVICE, scope)
    }
}
