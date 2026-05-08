use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use keyring::Entry;
use redacted::{Redacted, RedactContents};
use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "atlas-sh";
pub const TTL_HOURS: i64 = 8;

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedCredentials {
    pub username: String,
    #[serde(with = "redacted_serde")]
    pub password: Redacted<String, RedactContents>,
    #[serde(with = "redacted_serde")]
    pub connection_string: Redacted<String, RedactContents>,
    pub expires_at: DateTime<Utc>,
}

// redacted 0.2.0 imports `serde_bytes::Serialize` instead of `serde::Serialize`
// in its blanket impl, so `Redacted<String, _>` does not satisfy `serde::Serialize`.
// This module bridges the gap so the parent struct can use `#[derive(Serialize, Deserialize)]`.
mod redacted_serde {
    use super::{Redacted, RedactContents};

    pub fn serialize<S: serde::Serializer>(
        r: &Redacted<String, RedactContents>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&**r, s)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Redacted<String, RedactContents>, D::Error> {
        Ok(Redacted::new(<String as serde::Deserialize>::deserialize(d)?))
    }
}

impl CachedCredentials {
    pub fn new(username: String, password: String, connection_string: String) -> Self {
        Self {
            username,
            password: Redacted::new(password),
            connection_string: Redacted::new(connection_string),
            expires_at: Utc::now() + Duration::hours(TTL_HOURS),
        }
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }
}

/// Load cached credentials from the OS keychain.
/// Returns Ok(None) if no entry exists.
/// Returns Err if the keyring is unavailable (caller should degrade gracefully).
pub fn load(account: &str) -> Result<Option<CachedCredentials>> {
    let entry = Entry::new(KEYRING_SERVICE, account)
        .context("failed to open keyring entry")?;

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
pub fn store(account: &str, creds: &CachedCredentials) -> Result<()> {
    let entry = Entry::new(KEYRING_SERVICE, account)
        .context("failed to open keyring entry")?;
    let json = serde_json::to_string(creds).context("failed to serialize credentials")?;
    entry.set_password(&json).context("failed to write to keyring")?;
    Ok(())
}

/// Delete cached credentials from the OS keychain (best-effort).
/// Available for future cleanup tooling (e.g. `atlas sh logout`).
#[allow(dead_code)]
pub fn invalidate(account: &str) -> Result<()> {
    let entry = Entry::new(KEYRING_SERVICE, account)
        .context("failed to open keyring entry")?;
    entry.delete_credential().context("failed to delete keyring entry")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let creds = CachedCredentials::new(
            "atlas-sh-user".to_string(),
            "super-secret".to_string(),
            "mongodb+srv://cluster.abc.mongodb.net".to_string(),
        );
        let json = serde_json::to_string(&creds).unwrap();

        // Password must appear in the serialized form (keyring storage)
        assert!(json.contains("super-secret"), "password must serialize");
        assert!(json.contains("atlas-sh-user"));
        assert!(json.contains("mongodb+srv"));

        let decoded: CachedCredentials = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.username, "atlas-sh-user");
        assert_eq!(*decoded.password, "super-secret");
        assert_eq!(
            &**decoded.connection_string,
            "mongodb+srv://cluster.abc.mongodb.net"
        );
    }

    #[test]
    fn debug_redacts_secrets() {
        let creds = CachedCredentials::new(
            "user".to_string(),
            "topsecret".to_string(),
            "mongodb+srv://x".to_string(),
        );
        let debug = format!("{creds:?}");
        assert!(!debug.contains("topsecret"), "password must not appear in Debug");
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn is_expired_false_when_fresh() {
        let creds = CachedCredentials::new(
            "u".to_string(),
            "p".to_string(),
            "c".to_string(),
        );
        assert!(!creds.is_expired());
    }

    #[test]
    fn is_expired_true_when_past() {
        use chrono::Duration;
        let mut creds = CachedCredentials::new(
            "u".to_string(),
            "p".to_string(),
            "c".to_string(),
        );
        creds.expires_at = chrono::Utc::now() - Duration::seconds(1);
        assert!(creds.is_expired());
    }
}
