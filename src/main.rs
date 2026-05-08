use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use chrono::Duration;
use clap::Parser;
use mongodb_atlas_cli::atlas::client::AtlasClient;
use mongodb_atlas_cli::config::AtlasCLIConfig;
use rand::distributions::Alphanumeric;
use rand::Rng;
use uuid::Uuid;

mod args;
mod atlas_ops;
mod credentials;
mod deps;
mod domain;

use args::{Cli, ConnectionArgs, LogoutArgs, PluginSubCommands, ShArgs};
use credentials::CachedCredentials;
use deps::{AtlasApi, AtlasApiClient, Clock, CredentialStore, KeyringStore, SystemClock};
use domain::{ClusterName, ConnectionString, KeyringAccount, Password, ProjectId, Username};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    match Cli::parse().command {
        PluginSubCommands::Sh(args) => run_sh(args).await,
        PluginSubCommands::Logout(args) => run_logout(&args),
    }
}

// --- Subcommand entry points ----------------------------------------------

async fn run_sh(args: ShArgs) -> Result<()> {
    let client = build_client(&args.connection.profile)?;

    // Fail fast: find mongosh before any API calls.
    let mongosh_path = resolve_mongosh(client.config())?;
    tracing::debug!(path = %mongosh_path.display(), "found mongosh");

    let project_id = resolve_project_id(&args.connection, client.config())?;
    let cluster = ClusterName::from(args.connection.cluster.as_str());

    tracing::debug!(
        profile = %args.connection.profile,
        %project_id,
        %cluster,
        "resolved config",
    );

    let atlas = AtlasApiClient::new(&client);
    let credentials =
        obtain_credentials(&SystemClock, &KeyringStore, &atlas, &project_id, &cluster).await?;

    launch_mongosh(&mongosh_path, &credentials, &args.mongosh_args).map(|_: Infallible| ())
}

fn run_logout(args: &LogoutArgs) -> Result<()> {
    let client = build_client(&args.connection.profile)?;
    let project_id = resolve_project_id(&args.connection, client.config())?;
    let cluster = ClusterName::from(args.connection.cluster.as_str());

    let outcome = perform_logout(&KeyringStore, &project_id, &cluster)?;
    match outcome {
        LogoutOutcome::Removed => {
            tracing::info!(%project_id, %cluster, "removed cached credentials");
            println!(
                "Removed cached credentials for cluster '{cluster}' in project '{project_id}'."
            );
        }
        LogoutOutcome::NothingCached => {
            println!("No cached credentials for cluster '{cluster}' in project '{project_id}'.");
        }
    }
    Ok(())
}

// --- Orchestration (testable) ---------------------------------------------

/// Cache-or-create logic, abstracted from CLI parsing and process exec.
///
/// Returns currently-valid credentials, either from the keyring cache or by
/// provisioning a fresh temporary user via the Atlas API. Falls back to an
/// uncached creation when the keyring is unavailable so the user is never
/// blocked by a broken credential store.
async fn obtain_credentials<C, S, A>(
    clock: &C,
    store: &S,
    atlas: &A,
    project_id: &ProjectId,
    cluster: &ClusterName,
) -> Result<CachedCredentials>
where
    C: Clock,
    S: CredentialStore,
    A: AtlasApi,
{
    let account = KeyringAccount::new(project_id, cluster);
    match store.load(&account) {
        Ok(Some(creds)) if !creds.is_expired_at(clock.now()) => {
            tracing::info!(
                username = %creds.username,
                expires_at = %creds.expires_at,
                "using cached credentials",
            );
            Ok(creds)
        }
        Ok(cached) => {
            if cached.is_some() {
                tracing::info!("cached credentials expired, creating new user");
            } else {
                tracing::info!("no cached credentials, creating new user");
            }
            create_and_cache(clock, store, atlas, project_id, cluster, &account).await
        }
        Err(err) => {
            tracing::warn!(%err, "keyring unavailable, creating new user without caching");
            create_user(clock, atlas, project_id, cluster).await
        }
    }
}

/// Outcome of [`perform_logout`]; lets the caller pick a user-facing message.
#[derive(Debug, PartialEq, Eq)]
enum LogoutOutcome {
    Removed,
    NothingCached,
}

fn perform_logout<S: CredentialStore>(
    store: &S,
    project_id: &ProjectId,
    cluster: &ClusterName,
) -> Result<LogoutOutcome> {
    let account = KeyringAccount::new(project_id, cluster);
    if store.invalidate(&account)? {
        Ok(LogoutOutcome::Removed)
    } else {
        Ok(LogoutOutcome::NothingCached)
    }
}

async fn create_and_cache<C, S, A>(
    clock: &C,
    store: &S,
    atlas: &A,
    project_id: &ProjectId,
    cluster: &ClusterName,
    account: &KeyringAccount,
) -> Result<CachedCredentials>
where
    C: Clock,
    S: CredentialStore,
    A: AtlasApi,
{
    let creds = create_user(clock, atlas, project_id, cluster).await?;
    if let Err(err) = store.store(account, &creds) {
        tracing::warn!(%err, "failed to cache credentials in keyring");
    }
    Ok(creds)
}

async fn create_user<C, A>(
    clock: &C,
    atlas: &A,
    project_id: &ProjectId,
    cluster: &ClusterName,
) -> Result<CachedCredentials>
where
    C: Clock,
    A: AtlasApi,
{
    let srv = atlas.get_cluster_srv(project_id, cluster).await?;
    tracing::debug!(%srv, "got cluster SRV address");

    let username = Username::new(format!("atlas-sh-{}", Uuid::new_v4()));
    let password = generate_password();
    let expires_at = clock.now() + Duration::hours(credentials::TTL_HOURS);

    atlas
        .create_temp_db_user(project_id, &username, &password, &expires_at.to_rfc3339())
        .await?;

    tracing::info!(%username, %expires_at, "created temporary database user");

    Ok(CachedCredentials::new(
        username,
        password,
        ConnectionString::new(srv),
        expires_at,
    ))
}

/// Length of the generated random password. Atlas accepts a 4-128 character
/// range; 32 alphanumerics give ~190 bits of entropy.
const GENERATED_PASSWORD_LEN: usize = 32;

fn generate_password() -> Password {
    let raw: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(GENERATED_PASSWORD_LEN)
        .map(char::from)
        .collect();
    Password::new(raw)
}

// --- Process-level helpers ------------------------------------------------

fn build_client(profile: &str) -> Result<AtlasClient> {
    AtlasClient::with_profile(profile)
        .context("Failed to create Atlas client. Run 'atlas auth login' and try again.")
}

fn resolve_project_id(args: &ConnectionArgs, config: &AtlasCLIConfig) -> Result<ProjectId> {
    args.project_id
        .as_deref()
        .or(config.project_id.as_deref())
        .map(ProjectId::from)
        .ok_or_else(|| {
            anyhow!(
                "No project ID configured. Use --project-id or run \
                 'atlas config set project_id <id>'"
            )
        })
}

fn resolve_mongosh(config: &AtlasCLIConfig) -> Result<PathBuf> {
    if let Some(path) = &config.mongosh_path {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
        tracing::warn!(
            path = %p.display(),
            "mongosh_path from config does not exist, falling back to PATH",
        );
    }
    which::which("mongosh")
        .with_context(|| "mongosh not found. Install: https://www.mongodb.com/try/download/shell")
}

fn build_mongosh_command(
    mongosh_path: &Path,
    creds: &CachedCredentials,
    extra_args: &[String],
) -> Command {
    let mut cmd = Command::new(mongosh_path);
    cmd.arg(creds.connection_string.as_str())
        .args(["--username", creds.username.as_str()])
        .arg("--password")
        .arg(creds.password.as_str())
        .args(["--authenticationDatabase", "admin"])
        .args(extra_args);
    cmd
}

#[cfg(unix)]
fn launch_mongosh(
    mongosh_path: &Path,
    creds: &CachedCredentials,
    extra_args: &[String],
) -> Result<Infallible> {
    use std::os::unix::process::CommandExt;
    let err = build_mongosh_command(mongosh_path, creds, extra_args).exec();
    Err(anyhow!("Failed to exec mongosh: {err}"))
}

#[cfg(not(unix))]
fn launch_mongosh(
    mongosh_path: &Path,
    creds: &CachedCredentials,
    extra_args: &[String],
) -> Result<Infallible> {
    let status = build_mongosh_command(mongosh_path, creds, extra_args)
        .status()
        .context("Failed to launch mongosh")?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use deps::fakes::{FakeAtlasApi, FakeClock, MemoryStore};

    fn project() -> ProjectId {
        ProjectId::new("project-1")
    }

    fn cluster() -> ClusterName {
        ClusterName::new("MyCluster")
    }

    fn fixed_now() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 5, 8, 12, 0, 0).unwrap()
    }

    fn fresh_creds(expires_at: chrono::DateTime<chrono::Utc>) -> CachedCredentials {
        CachedCredentials::new(
            Username::new("atlas-sh-cached"),
            Password::new("cached-pw"),
            ConnectionString::new("mongodb+srv://cached.mongodb.net"),
            expires_at,
        )
    }

    #[tokio::test]
    async fn obtain_credentials_returns_cache_hit_when_unexpired() {
        let now = fixed_now();
        let clock = FakeClock::new(now);
        let cached = fresh_creds(now + Duration::hours(1));
        let account = KeyringAccount::new(&project(), &cluster());
        let store = MemoryStore::with_entry(&account, fresh_creds(now + Duration::hours(1)));
        let atlas = FakeAtlasApi::with_srv("mongodb+srv://fresh.mongodb.net");

        let creds = obtain_credentials(&clock, &store, &atlas, &project(), &cluster())
            .await
            .unwrap();

        assert_eq!(creds.username.as_str(), cached.username.as_str());
        assert!(atlas.created_users.borrow().is_empty());
    }

    #[tokio::test]
    async fn obtain_credentials_creates_new_when_cache_expired() {
        let now = fixed_now();
        let clock = FakeClock::new(now);
        let account = KeyringAccount::new(&project(), &cluster());
        let store = MemoryStore::with_entry(&account, fresh_creds(now - Duration::seconds(1)));
        let atlas = FakeAtlasApi::with_srv("mongodb+srv://fresh.mongodb.net");

        let creds = obtain_credentials(&clock, &store, &atlas, &project(), &cluster())
            .await
            .unwrap();

        assert!(creds.username.as_str().starts_with("atlas-sh-"));
        assert_ne!(creds.username.as_str(), "atlas-sh-cached");
        assert_eq!(atlas.created_users.borrow().len(), 1);
        assert!(store.contains(&account));
    }

    #[tokio::test]
    async fn obtain_credentials_creates_and_caches_on_miss() {
        let clock = FakeClock::new(fixed_now());
        let store = MemoryStore::default();
        let atlas = FakeAtlasApi::with_srv("mongodb+srv://fresh.mongodb.net");

        let creds = obtain_credentials(&clock, &store, &atlas, &project(), &cluster())
            .await
            .unwrap();

        assert_eq!(
            creds.connection_string.as_str(),
            "mongodb+srv://fresh.mongodb.net"
        );
        assert_eq!(atlas.created_users.borrow().len(), 1);
        assert!(store.contains(&KeyringAccount::new(&project(), &cluster())));
    }

    #[tokio::test]
    async fn obtain_credentials_skips_caching_when_keyring_fails() {
        let clock = FakeClock::new(fixed_now());
        let store = MemoryStore::default().fail_load();
        let atlas = FakeAtlasApi::with_srv("mongodb+srv://fresh.mongodb.net");

        let creds = obtain_credentials(&clock, &store, &atlas, &project(), &cluster())
            .await
            .unwrap();

        assert_eq!(
            creds.connection_string.as_str(),
            "mongodb+srv://fresh.mongodb.net"
        );
        assert_eq!(atlas.created_users.borrow().len(), 1);
        // Keyring is broken: nothing was stored.
        assert!(!store.contains(&KeyringAccount::new(&project(), &cluster())));
    }

    #[tokio::test]
    async fn obtain_credentials_treats_exact_expiry_as_expired() {
        let now = fixed_now();
        let clock = FakeClock::new(now);
        let account = KeyringAccount::new(&project(), &cluster());
        // Exact boundary: expires_at == now, `>=` makes it expired.
        let store = MemoryStore::with_entry(&account, fresh_creds(now));
        let atlas = FakeAtlasApi::with_srv("mongodb+srv://fresh.mongodb.net");

        let creds = obtain_credentials(&clock, &store, &atlas, &project(), &cluster())
            .await
            .unwrap();

        assert_ne!(creds.username.as_str(), "atlas-sh-cached");
        assert_eq!(atlas.created_users.borrow().len(), 1);
    }

    #[test]
    fn perform_logout_returns_removed_when_entry_existed() {
        let account = KeyringAccount::new(&project(), &cluster());
        let store = MemoryStore::with_entry(&account, fresh_creds(fixed_now()));

        let outcome = perform_logout(&store, &project(), &cluster()).unwrap();

        assert_eq!(outcome, LogoutOutcome::Removed);
        assert!(!store.contains(&account));
    }

    #[test]
    fn perform_logout_returns_nothing_cached_on_empty_store() {
        let store = MemoryStore::default();

        let outcome = perform_logout(&store, &project(), &cluster()).unwrap();

        assert_eq!(outcome, LogoutOutcome::NothingCached);
    }

    #[test]
    fn generated_password_has_expected_length_and_charset() {
        let pw = generate_password();
        let s = pw.as_str();
        assert_eq!(s.len(), GENERATED_PASSWORD_LEN);
        assert!(s.bytes().all(|b| b.is_ascii_alphanumeric()));
    }
}
