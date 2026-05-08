//! Dependency abstractions for the orchestration layer.
//!
//! `run_sh` and `run_logout` ultimately need three side-effecting capabilities:
//!
//! 1. **Reading the wall clock** — to decide whether cached credentials have
//!    expired and to stamp newly minted credentials.
//! 2. **Reading and writing the OS keychain** — credential cache.
//! 3. **Talking to the Atlas API** — fetching SRV records and creating users.
//!
//! Each is exposed as a small trait so the orchestration logic can be exercised
//! against in-memory fakes in unit tests. Production code wires them to
//! [`SystemClock`], [`KeyringStore`], and [`AtlasApiClient`] respectively.

use std::future::Future;

use anyhow::Result;
use chrono::{DateTime, Utc};
use mongodb_atlas_cli::atlas::client::AtlasClient;

use crate::atlas_ops;
use crate::credentials::{self, CachedCredentials};
use crate::domain::{ClusterName, KeyringAccount, Password, ProjectId, Username};

// --- Clock -----------------------------------------------------------------

/// Reads the current time. Production uses [`SystemClock`]; tests use a fake.
pub(crate) trait Clock {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

// --- CredentialStore -------------------------------------------------------

/// Persists [`CachedCredentials`] keyed by [`KeyringAccount`].
///
/// `load` returns `Ok(None)` for a missing entry and `Err` for a real
/// failure; `invalidate` returns `Ok(true)` when an entry was removed and
/// `Ok(false)` when nothing was cached. The contract matches the free
/// functions in [`crate::credentials`].
pub(crate) trait CredentialStore {
    fn load(&self, account: &KeyringAccount) -> Result<Option<CachedCredentials>>;
    fn store(&self, account: &KeyringAccount, creds: &CachedCredentials) -> Result<()>;
    fn invalidate(&self, account: &KeyringAccount) -> Result<bool>;
}

/// Production keyring-backed store. Thin wrapper over the free functions in
/// [`crate::credentials`].
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KeyringStore;

impl CredentialStore for KeyringStore {
    fn load(&self, account: &KeyringAccount) -> Result<Option<CachedCredentials>> {
        credentials::load(account)
    }

    fn store(&self, account: &KeyringAccount, creds: &CachedCredentials) -> Result<()> {
        credentials::store(account, creds)
    }

    fn invalidate(&self, account: &KeyringAccount) -> Result<bool> {
        credentials::invalidate(account)
    }
}

// --- AtlasApi --------------------------------------------------------------

/// Subset of the Atlas REST API used by this plugin.
///
/// Uses stable `async fn` in trait (RPITIT). The trait isn't object-safe but
/// the call sites all use static dispatch via generics, so that's fine.
pub(crate) trait AtlasApi {
    fn get_cluster_srv(
        &self,
        project_id: &ProjectId,
        cluster: &ClusterName,
    ) -> impl Future<Output = Result<String>>;

    fn create_temp_db_user(
        &self,
        project_id: &ProjectId,
        username: &Username,
        password: &Password,
        delete_after_date: &str,
    ) -> impl Future<Output = Result<()>>;
}

/// Production implementation backed by the upstream [`AtlasClient`].
pub(crate) struct AtlasApiClient<'a> {
    client: &'a AtlasClient,
}

impl<'a> AtlasApiClient<'a> {
    pub(crate) const fn new(client: &'a AtlasClient) -> Self {
        Self { client }
    }
}

impl AtlasApi for AtlasApiClient<'_> {
    async fn get_cluster_srv(
        &self,
        project_id: &ProjectId,
        cluster: &ClusterName,
    ) -> Result<String> {
        atlas_ops::get_cluster_srv(self.client, project_id, cluster).await
    }

    async fn create_temp_db_user(
        &self,
        project_id: &ProjectId,
        username: &Username,
        password: &Password,
        delete_after_date: &str,
    ) -> Result<()> {
        atlas_ops::create_temp_db_user(
            self.client,
            project_id,
            username,
            password,
            delete_after_date,
        )
        .await
    }
}

// --- In-memory test doubles ------------------------------------------------

#[cfg(test)]
pub(crate) mod fakes {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    /// Test clock returning a pinned [`DateTime<Utc>`].
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct FakeClock(DateTime<Utc>);

    impl FakeClock {
        pub(crate) const fn new(now: DateTime<Utc>) -> Self {
            Self(now)
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// In-memory [`CredentialStore`] for tests. Records every call so tests
    /// can assert on store/invalidate side effects.
    #[derive(Debug, Default)]
    pub(crate) struct MemoryStore {
        entries: RefCell<HashMap<String, CachedCredentials>>,
        pub(crate) load_fails: RefCell<bool>,
    }

    impl MemoryStore {
        pub(crate) fn with_entry(account: &KeyringAccount, creds: CachedCredentials) -> Self {
            let s = Self::default();
            s.entries
                .borrow_mut()
                .insert(account.as_str().to_owned(), creds);
            s
        }

        pub(crate) fn fail_load(self) -> Self {
            *self.load_fails.borrow_mut() = true;
            self
        }

        pub(crate) fn contains(&self, account: &KeyringAccount) -> bool {
            self.entries.borrow().contains_key(account.as_str())
        }
    }

    impl CredentialStore for MemoryStore {
        fn load(&self, account: &KeyringAccount) -> Result<Option<CachedCredentials>> {
            if *self.load_fails.borrow() {
                return Err(anyhow::anyhow!("simulated keyring failure"));
            }
            Ok(self.entries.borrow().get(account.as_str()).map(clone_creds))
        }

        fn store(&self, account: &KeyringAccount, creds: &CachedCredentials) -> Result<()> {
            self.entries
                .borrow_mut()
                .insert(account.as_str().to_owned(), clone_creds(creds));
            Ok(())
        }

        fn invalidate(&self, account: &KeyringAccount) -> Result<bool> {
            Ok(self.entries.borrow_mut().remove(account.as_str()).is_some())
        }
    }

    /// `CachedCredentials` doesn't impl `Clone` because secrets shouldn't
    /// proliferate. The fake store needs to hand out copies for assertions,
    /// so we go through serde — same on-wire format the keyring would use.
    fn clone_creds(creds: &CachedCredentials) -> CachedCredentials {
        let json = serde_json::to_string(creds).expect("CachedCredentials should serialize");
        serde_json::from_str(&json).expect("round-trip should deserialize")
    }

    /// Scripted [`AtlasApi`]. Records every call.
    #[derive(Debug, Default)]
    pub(crate) struct FakeAtlasApi {
        pub(crate) srv: String,
        pub(crate) created_users: RefCell<Vec<(Username, Password, String)>>,
    }

    impl FakeAtlasApi {
        pub(crate) fn with_srv(srv: impl Into<String>) -> Self {
            Self {
                srv: srv.into(),
                created_users: RefCell::default(),
            }
        }
    }

    impl AtlasApi for FakeAtlasApi {
        async fn get_cluster_srv(
            &self,
            _project_id: &ProjectId,
            _cluster: &ClusterName,
        ) -> Result<String> {
            Ok(self.srv.clone())
        }

        async fn create_temp_db_user(
            &self,
            _project_id: &ProjectId,
            username: &Username,
            password: &Password,
            delete_after_date: &str,
        ) -> Result<()> {
            self.created_users.borrow_mut().push((
                username.clone(),
                password.clone(),
                delete_after_date.to_owned(),
            ));
            Ok(())
        }
    }
}
