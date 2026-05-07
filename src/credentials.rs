use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use keyring::Entry;
use redacted::{Redacted, RedactContents};
use serde::{Deserialize, Serialize, Serializer, Deserializer};

const KEYRING_SERVICE: &str = "atlas-sh";
pub const TTL_HOURS: i64 = 8;

#[derive(Debug)]
pub struct CachedCredentials {
    pub username: String,
    pub password: Redacted<String, RedactContents>,
    pub connection_string: Redacted<String, RedactContents>,
    pub expires_at: DateTime<Utc>,
}

impl Serialize for CachedCredentials {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("CachedCredentials", 4)?;
        state.serialize_field("username", &self.username)?;
        state.serialize_field("password", &**self.password)?;
        state.serialize_field("connection_string", &**self.connection_string)?;
        state.serialize_field("expires_at", &self.expires_at)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for CachedCredentials {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        #[derive(Deserialize)]
        #[serde(field_identifier)]
        enum Field {
            #[serde(rename = "username")]
            Username,
            #[serde(rename = "password")]
            Password,
            #[serde(rename = "connection_string")]
            ConnectionString,
            #[serde(rename = "expires_at")]
            ExpiresAt,
        }

        struct CachedCredentialsVisitor;

        impl<'de> Visitor<'de> for CachedCredentialsVisitor {
            type Value = CachedCredentials;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct CachedCredentials")
            }

            fn visit_map<V>(self, mut map: V) -> Result<CachedCredentials, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut username = None;
                let mut password = None;
                let mut connection_string = None;
                let mut expires_at = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Username => {
                            if username.is_some() {
                                return Err(de::Error::duplicate_field("username"));
                            }
                            username = Some(map.next_value()?);
                        }
                        Field::Password => {
                            if password.is_some() {
                                return Err(de::Error::duplicate_field("password"));
                            }
                            password = Some(map.next_value()?);
                        }
                        Field::ConnectionString => {
                            if connection_string.is_some() {
                                return Err(de::Error::duplicate_field("connection_string"));
                            }
                            connection_string = Some(map.next_value()?);
                        }
                        Field::ExpiresAt => {
                            if expires_at.is_some() {
                                return Err(de::Error::duplicate_field("expires_at"));
                            }
                            expires_at = Some(map.next_value()?);
                        }
                    }
                }

                let username = username.ok_or_else(|| de::Error::missing_field("username"))?;
                let password: String =
                    password.ok_or_else(|| de::Error::missing_field("password"))?;
                let connection_string: String = connection_string
                    .ok_or_else(|| de::Error::missing_field("connection_string"))?;
                let expires_at =
                    expires_at.ok_or_else(|| de::Error::missing_field("expires_at"))?;

                Ok(CachedCredentials {
                    username,
                    password: Redacted::new(password),
                    connection_string: Redacted::new(connection_string),
                    expires_at,
                })
            }
        }

        deserializer.deserialize_struct(
            "CachedCredentials",
            &["username", "password", "connection_string", "expires_at"],
            CachedCredentialsVisitor,
        )
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
