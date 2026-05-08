use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Utc};
use clap::Parser;
use mongodb_atlas_cli::atlas::client::AtlasClient;
use mongodb_atlas_cli::config::AtlasCLIConfig;
use rand::distributions::Alphanumeric;
use rand::Rng;
use uuid::Uuid;

mod args;
mod atlas_ops;
mod credentials;

use args::{Cli, ConnectionArgs, LogoutArgs, PluginSubCommands, ShArgs};
use credentials::CachedCredentials;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    match Cli::parse().command {
        PluginSubCommands::Sh(args) => run_sh(args).await,
        PluginSubCommands::Logout(args) => run_logout(&args),
    }
}

async fn run_sh(args: ShArgs) -> Result<()> {
    let client = build_client(&args.connection.profile)?;

    // Fail fast: find mongosh before any API calls.
    let mongosh_path = resolve_mongosh(client.config())?;
    tracing::debug!(path = %mongosh_path.display(), "found mongosh");

    let project_id = resolve_project_id(&args.connection, client.config())?;
    let cluster = &args.connection.cluster;

    tracing::debug!(
        profile = %args.connection.profile,
        %project_id,
        %cluster,
        "resolved config",
    );

    let keyring_account = keyring_account(&project_id, cluster);
    let credentials = match credentials::load(&keyring_account) {
        Ok(Some(creds)) if !creds.is_expired() => {
            tracing::info!(
                username = %creds.username,
                expires_at = %creds.expires_at,
                "using cached credentials",
            );
            creds
        }
        Ok(cached) => {
            if cached.is_some() {
                tracing::info!("cached credentials expired, creating new user");
            } else {
                tracing::info!("no cached credentials, creating new user");
            }
            create_and_cache_user(&client, &project_id, cluster, &keyring_account).await?
        }
        Err(err) => {
            tracing::warn!(%err, "keyring unavailable, creating new user without caching");
            create_user_uncached(&client, &project_id, cluster).await?
        }
    };

    launch_mongosh(&mongosh_path, &credentials, &args.mongosh_args).map(|_: Infallible| ())
}

fn run_logout(args: &LogoutArgs) -> Result<()> {
    let client = build_client(&args.connection.profile)?;
    let project_id = resolve_project_id(&args.connection, client.config())?;
    let cluster = &args.connection.cluster;
    let account = keyring_account(&project_id, cluster);

    if credentials::invalidate(&account)? {
        tracing::info!(%project_id, %cluster, "removed cached credentials");
        println!("Removed cached credentials for cluster '{cluster}' in project '{project_id}'.");
    } else {
        println!("No cached credentials for cluster '{cluster}' in project '{project_id}'.");
    }
    Ok(())
}

fn build_client(profile: &str) -> Result<AtlasClient> {
    AtlasClient::with_profile(profile)
        .context("Failed to create Atlas client. Run 'atlas auth login' and try again.")
}

fn resolve_project_id(args: &ConnectionArgs, config: &AtlasCLIConfig) -> Result<String> {
    args.project_id
        .as_deref()
        .or(config.project_id.as_deref())
        .map(str::to_owned)
        .ok_or_else(|| {
            anyhow!(
                "No project ID configured. Use --project-id or run \
                 'atlas config set project_id <id>'"
            )
        })
}

fn keyring_account(project_id: &str, cluster: &str) -> String {
    format!("{project_id}:{cluster}")
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

async fn create_and_cache_user(
    client: &AtlasClient,
    project_id: &str,
    cluster: &str,
    keyring_account: &str,
) -> Result<CachedCredentials> {
    let creds = create_user_uncached(client, project_id, cluster).await?;
    if let Err(err) = credentials::store(keyring_account, &creds) {
        tracing::warn!(%err, "failed to cache credentials in keyring");
    }
    Ok(creds)
}

async fn create_user_uncached(
    client: &AtlasClient,
    project_id: &str,
    cluster: &str,
) -> Result<CachedCredentials> {
    let srv = atlas_ops::get_cluster_srv(client, project_id, cluster).await?;
    tracing::debug!(%srv, "got cluster SRV address");

    let username = format!("atlas-sh-{}", Uuid::new_v4());
    let password: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    let expires_at = Utc::now() + Duration::hours(credentials::TTL_HOURS);
    atlas_ops::create_temp_db_user(
        client,
        project_id,
        &username,
        &password,
        &expires_at.to_rfc3339(),
    )
    .await?;

    tracing::info!(%username, %expires_at, "created temporary database user");

    Ok(CachedCredentials::new(username, password, srv, expires_at))
}

fn build_mongosh_command(
    mongosh_path: &Path,
    creds: &CachedCredentials,
    extra_args: &[String],
) -> Command {
    let mut cmd = Command::new(mongosh_path);
    cmd.arg(&**creds.connection_string)
        .args(["--username", &creds.username])
        .arg("--password")
        .arg(&**creds.password)
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
