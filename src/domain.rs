//! Domain primitives for the plugin.
//!
//! Each type wraps a `String` (or `Redacted<String>` for secrets) so that
//! adjacent string-typed parameters cannot be swapped at call sites without a
//! compile error. The wrappers are intentionally transparent for serde so the
//! on-disk and on-wire formats stay identical to plain strings.
//!
//! Construction is lossless: any `String` can become any newtype. The value
//! is in *flow* — once a value is wrapped, it cannot accidentally cross over
//! to a sibling type without an explicit conversion.

use std::fmt;
use std::str::FromStr;

use redacted::{RedactContents, Redacted};
use serde::{Deserialize, Serialize};

// --- Plain string newtypes --------------------------------------------------

macro_rules! string_newtype {
    ($(#[$meta:meta])* $vis:vis struct $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, Hash,
            ::serde::Serialize, ::serde::Deserialize,
        )]
        #[serde(transparent)]
        $vis struct $name(String);

        impl $name {
            #[allow(dead_code)]
            $vis fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            $vis fn as_str(&self) -> &str {
                &self.0
            }

            #[allow(dead_code)]
            $vis fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl FromStr for $name {
            type Err = ::std::convert::Infallible;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.to_owned()))
            }
        }
    };
}

string_newtype!(
    /// Atlas project (group) identifier.
    pub(crate) struct ProjectId
);

string_newtype!(
    /// Name of a cluster within an Atlas project.
    pub(crate) struct ClusterName
);

string_newtype!(
    /// Database username for authenticating against an Atlas cluster.
    pub(crate) struct Username
);

// --- Composite key ---------------------------------------------------------

/// Composite key used to look up cached credentials in the OS keychain.
///
/// Format: `<project_id>:<cluster_name>`. The format is stable on disk and
/// must not change without a migration plan.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct KeyringAccount(String);

impl KeyringAccount {
    pub(crate) fn new(project_id: &ProjectId, cluster: &ClusterName) -> Self {
        Self(format!("{project_id}:{cluster}"))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyringAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// --- Secret newtypes -------------------------------------------------------

/// Database password.
///
/// Wraps `Redacted<String, RedactContents>` so the value never leaks via
/// `Debug` (`{password:?}` prints `REDACTED`). Serde delegates straight to
/// the inner `String`, eliminating the need for a `#[serde(with = …)]`
/// bridge module.
#[derive(Debug, Clone)]
pub(crate) struct Password(Redacted<String, RedactContents>);

/// Mongo connection string (mongodb+srv://…) supplied by Atlas.
///
/// Same redaction guarantees as [`Password`]; defence-in-depth so the URL —
/// which can include credentials when fully populated — never appears in
/// log output.
#[derive(Debug, Clone)]
pub(crate) struct ConnectionString(Redacted<String, RedactContents>);

// Secrets do *not* implement `Deref<Target = str>` or `Display`. Accessing the
// inner value requires an explicit `as_str()` call so leaks have a visible
// surface in code review. `Debug` is provided by `Redacted` and prints
// `REDACTED` regardless.
macro_rules! redacted_string_newtype_serde {
    ($name:ident) => {
        impl $name {
            pub(crate) fn new(s: impl Into<String>) -> Self {
                Self(Redacted::new(s.into()))
            }

            pub(crate) fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                Serialize::serialize(&*self.0, s)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                String::deserialize(d).map(Self::new)
            }
        }
    };
}

redacted_string_newtype_serde!(Password);
redacted_string_newtype_serde!(ConnectionString);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_displays_inner_value() {
        let id = ProjectId::new("abc123");
        assert_eq!(id.to_string(), "abc123");
        assert_eq!(id.as_str(), "abc123");
        assert_eq!(format!("{id}"), "abc123");
    }

    #[test]
    fn cluster_name_round_trips_through_serde_transparent() {
        let original = ClusterName::new("MyCluster");
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#""MyCluster""#);
        let decoded: ClusterName = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn username_from_str_is_infallible() {
        let u: Username = "atlas-sh-test".parse().unwrap();
        assert_eq!(u.as_str(), "atlas-sh-test");
    }

    #[test]
    fn keyring_account_format_is_stable() {
        let account = KeyringAccount::new(&ProjectId::new("5f1b"), &ClusterName::new("MyCluster"));
        assert_eq!(account.as_str(), "5f1b:MyCluster");
    }

    #[test]
    fn password_debug_redacts() {
        let p = Password::new("topsecret");
        let debug = format!("{p:?}");
        assert!(!debug.contains("topsecret"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn password_serializes_as_string() {
        let p = Password::new("topsecret");
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, r#""topsecret""#);
        let decoded: Password = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.as_str(), "topsecret");
    }

    #[test]
    fn connection_string_serializes_as_string() {
        let cs = ConnectionString::new("mongodb+srv://x.mongodb.net");
        let json = serde_json::to_string(&cs).unwrap();
        assert_eq!(json, r#""mongodb+srv://x.mongodb.net""#);
        let decoded: ConnectionString = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.as_str(), "mongodb+srv://x.mongodb.net");
    }

    /// Compile-fence: a `ProjectId` cannot silently be passed where a
    /// `ClusterName` is expected. This is the structural value of having
    /// distinct newtypes; the test only documents the property — the type
    /// system enforces it at every call site.
    #[test]
    fn newtypes_are_distinct_at_the_type_level() {
        let p = ProjectId::new("p");
        let c = ClusterName::new("c");
        // Compiles only because `as_str()` returns `&str` for both.
        let _: &str = p.as_str();
        let _: &str = c.as_str();
        // The following would fail to compile (verified manually):
        //   let _: ClusterName = p;
    }
}
