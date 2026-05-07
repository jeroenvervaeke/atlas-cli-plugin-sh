use anyhow::{Context, Result, anyhow};
use chrono::{Duration, Utc};
use clap::Parser;
use rand::Rng;
use uuid::Uuid;

mod args;
mod atlas_ops;
mod credentials;

use args::{Cli, PluginSubCommands};
use redacted::{Redacted, RedactContents};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let PluginSubCommands::Sh(args) = cli.command;

    // 1. Load Atlas config and build client
    let client = mongodb_atlas_cli::atlas::client::AtlasClient::with_profile(&args.profile)
        .context("Failed to create Atlas client. Run 'atlas auth login' and try again.")?;

    // 2. Fail fast: find mongosh before any API calls
    let mongosh_path = resolve_mongosh(client.config())?;
    tracing::debug!(path = %mongosh_path.display(), "found mongosh");

    // 3. Resolve project_id (flag overrides config)
    let project_id = args
        .project_id
        .clone()
        .or_else(|| client.config().project_id.clone())
        .ok_or_else(|| {
            anyhow!(
                "No project ID configured. Use --project-id or run 'atlas config set project_id <id>'"
            )
        })?;

    tracing::debug!(
        profile = %args.profile,
        project_id = %project_id,
        cluster = %args.cluster,
        "resolved config"
    );

    // 4. Check keyring cache
    let keyring_account = format!("{}:{}", project_id, args.cluster);
    let credentials = match credentials::load(&keyring_account) {
        Ok(Some(creds)) if !creds.is_expired() => {
            tracing::info!(
                username = %creds.username,
                expires_at = %creds.expires_at,
                "using cached credentials"
            );
            creds
        }
        Ok(cached) => {
            if cached.is_some() {
                tracing::info!("cached credentials expired, creating new user");
            } else {
                tracing::info!("no cached credentials, creating new user");
            }
            create_and_cache_user(&client, &project_id, &args.cluster, &keyring_account).await?
        }
        Err(e) => {
            tracing::warn!(err = %e, "keyring unavailable, creating new user without caching");
            create_user_uncached(&client, &project_id, &args.cluster).await?
        }
    };

    // 5. Exec mongosh (replaces current process)
    launch_mongosh(
        &mongosh_path,
        &credentials.connection_string,
        &credentials.username,
        &credentials.password,
        &args.mongosh_args,
    )
}

fn resolve_mongosh(config: &mongodb_atlas_cli::config::AtlasCLIConfig) -> Result<std::path::PathBuf> {
    if let Some(path) = &config.mongosh_path {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
        tracing::warn!(
            path = %p.display(),
            "mongosh_path from config does not exist, falling back to PATH"
        );
    }
    which::which("mongosh").map_err(|_| {
        anyhow!(
            "mongosh not found. Install: https://www.mongodb.com/try/download/shell"
        )
    })
}

async fn create_and_cache_user(
    client: &mongodb_atlas_cli::atlas::client::AtlasClient,
    project_id: &str,
    cluster: &str,
    keyring_account: &str,
) -> Result<credentials::CachedCredentials> {
    let creds = create_user_uncached(client, project_id, cluster).await?;
    if let Err(e) = credentials::store(keyring_account, &creds) {
        tracing::warn!(err = %e, "failed to cache credentials in keyring");
    }
    Ok(creds)
}

async fn create_user_uncached(
    client: &mongodb_atlas_cli::atlas::client::AtlasClient,
    project_id: &str,
    cluster: &str,
) -> Result<credentials::CachedCredentials> {
    let srv = atlas_ops::get_cluster_srv(client, project_id, cluster).await?;
    tracing::debug!(srv = %srv, "got cluster SRV address");

    let username = format!("atlas-sh-{}", Uuid::new_v4());
    let password: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    let expires_at = Utc::now() + Duration::hours(credentials::TTL_HOURS);
    let delete_after_date = expires_at.to_rfc3339();

    atlas_ops::create_temp_db_user(client, project_id, &username, &password, &delete_after_date)
        .await?;

    tracing::info!(username = %username, expires_at = %expires_at, "created temporary database user");

    Ok(credentials::CachedCredentials::new(username, password, srv))
}

fn launch_mongosh(
    mongosh_path: &std::path::Path,
    connection_string: &Redacted<String, RedactContents>,
    username: &str,
    password: &Redacted<String, RedactContents>,
    extra_args: &[String],
) -> Result<()> {
    let mut cmd = std::process::Command::new(mongosh_path);
    cmd.arg(&**connection_string)
        .arg("--username")
        .arg(username)
        .arg("--password")
        .arg(&**password)
        .arg("--authenticationDatabase")
        .arg("admin");

    for arg in extra_args {
        cmd.arg(arg);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        return Err(anyhow!("Failed to exec mongosh: {err}"));
    }

    #[cfg(not(unix))]
    {
        let status = cmd.status().context("Failed to launch mongosh")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
