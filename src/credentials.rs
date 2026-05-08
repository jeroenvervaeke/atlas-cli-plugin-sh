use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::domain::{ConnectionString, KeyringAccount, Password, Username};

const KEYRING_SERVICE: &str = "atlas-sh";
pub(crate) const TTL_HOURS: i64 = 8;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CachedCredentials {
    pub(crate) username: Username,
    pub(crate) password: Password,
    pub(crate) connection_string: ConnectionString,
    pub(crate) expires_at: DateTime<Utc>,
}

impl CachedCredentials {
    pub(crate) const fn new(
        username: Username,
        password: Password,
        connection_string: ConnectionString,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            username,
            password,
            connection_string,
            expires_at,
        }
    }

    pub(crate) fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }
}

/// Load cached credentials from the OS keychain.
/// Returns Ok(None) if no entry exists.
/// Returns Err if the keyring is unavailable (caller should degrade gracefully).
pub(crate) fn load(account: &KeyringAccount) -> Result<Option<CachedCredentials>> {
    let entry =
        Entry::new(KEYRING_SERVICE, account.as_str()).context("failed to open keyring entry")?;

    match entry.get_password() {
        Ok(json) => {
            let creds: CachedCredentials =
                serde_json::from_str(&json).context("failed to parse cached credentials")?;
            Ok(Some(creds))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("keyring error: {e}")),
    }
}

/// Store credentials in the OS keychain.
pub(crate) fn store(account: &KeyringAccount, creds: &CachedCredentials) -> Result<()> {
    let entry =
        Entry::new(KEYRING_SERVICE, account.as_str()).context("failed to open keyring entry")?;
    let json = serde_json::to_string(creds).context("failed to serialize credentials")?;
    entry
        .set_password(&json)
        .context("failed to write to keyring")?;
    Ok(())
}

/// Delete cached credentials from the OS keychain.
///
/// Returns `Ok(true)` when an entry was removed and `Ok(false)` when nothing
/// was cached for `account` (idempotent — calling logout twice is not an
/// error). Returns `Err` for genuine keyring failures.
pub(crate) fn invalidate(account: &KeyringAccount) -> Result<bool> {
    let entry =
        Entry::new(KEYRING_SERVICE, account.as_str()).context("failed to open keyring entry")?;
    match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(anyhow::anyhow!("failed to delete keyring entry: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ClusterName, ProjectId};

    fn fresh_creds() -> CachedCredentials {
        CachedCredentials::new(
            Username::new("atlas-sh-user"),
            Password::new("super-secret"),
            ConnectionString::new("mongodb+srv://cluster.abc.mongodb.net"),
            Utc::now() + chrono::Duration::hours(TTL_HOURS),
        )
    }

    #[test]
    fn round_trips_through_json() {
        let creds = fresh_creds();
        let json = serde_json::to_string(&creds).unwrap();

        // Password must appear in the serialized form (keyring storage).
        assert!(json.contains("super-secret"), "password must serialize");
        assert!(json.contains("atlas-sh-user"));
        assert!(json.contains("mongodb+srv"));

        let decoded: CachedCredentials = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.username.as_str(), "atlas-sh-user");
        assert_eq!(decoded.password.as_str(), "super-secret");
        assert_eq!(
            decoded.connection_string.as_str(),
            "mongodb+srv://cluster.abc.mongodb.net"
        );
    }

    #[test]
    fn debug_redacts_secrets() {
        let creds = fresh_creds();
        let debug = format!("{creds:?}");
        assert!(
            !debug.contains("super-secret"),
            "password must not appear in Debug",
        );
        assert!(
            !debug.contains("mongodb+srv"),
            "connection string must not appear in Debug",
        );
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn is_expired_false_when_fresh() {
        let mut creds = fresh_creds();
        creds.expires_at = Utc::now() + chrono::Duration::hours(1);
        assert!(!creds.is_expired());
    }

    #[test]
    fn is_expired_true_when_past() {
        let mut creds = fresh_creds();
        creds.expires_at = Utc::now() - chrono::Duration::seconds(1);
        assert!(creds.is_expired());
    }

    #[test]
    fn keyring_account_passes_through_to_underlying_apis() {
        let account = KeyringAccount::new(&ProjectId::new("p"), &ClusterName::new("c"));
        // We cannot exercise the real keyring in unit tests without flakiness
        // on different platforms; the assertion documents the format the
        // keyring sees.
        assert_eq!(account.as_str(), "p:c");
    }
}
