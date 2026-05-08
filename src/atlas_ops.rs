use anyhow::{anyhow, Error, Result};
use http::StatusCode;
use mongodb_atlas_cli::atlas::client::AtlasClient;
use mongodb_atlas_cli::atlas::layer::OperationError;
use mongodb_atlas_cli::atlas::operation;
use serde::{Deserialize, Serialize};

use crate::domain::{ClusterName, Password, ProjectId, Username};

/// Database the temporary user authenticates against.
const AUTH_DATABASE: &str = "admin";

/// Roles granted to the temporary user.
const TEMP_USER_ROLES: &[(&str, &str)] = &[("readWriteAnyDatabase", AUTH_DATABASE)];

// --- GetCluster ---

/// `GET /api/atlas/v2/groups/{group_id}/clusters/{cluster_name}`
#[derive(Debug)]
#[operation(method = GET, version = "2024-08-05")]
#[url("/api/atlas/v2/groups/{group_id}/clusters/{cluster_name}")]
#[response(ClusterDetail)]
struct GetClusterRequest {}

// Must be `pub` (not `pub(crate)`): the `#[operation]` macro generates a public
// alias referencing this type, so restricting visibility triggers E0446.
#[allow(unreachable_pub)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterDetail {
    pub(crate) connection_strings: ConnectionStrings,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionStrings {
    pub(crate) standard_srv: String,
}

// --- CreateDatabaseUser ---

/// `POST /api/atlas/v2/groups/{group_id}/databaseUsers`
//
// Per-field `#[serde(rename = "…")]` is required: the `#[operation]` macro
// generates the `Serialize` derive itself, so a struct-level
// `#[serde(rename_all)]` is not visible at expansion time.
#[derive(Debug)]
#[operation(method = POST, version = "2024-08-05")]
#[url("/api/atlas/v2/groups/{group_id}/databaseUsers")]
#[response(DatabaseUserResponse)]
struct CreateDatabaseUserRequest {
    #[serde(rename = "databaseName")]
    database_name: String,
    username: String,
    password: String,
    roles: Vec<DatabaseUserRole>,
    #[serde(rename = "deleteAfterDate")]
    delete_after_date: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DatabaseUserRole {
    pub(crate) role_name: String,
    pub(crate) database_name: String,
}

// See note on `ClusterDetail`: must be `pub` for the `#[operation]` macro.
#[allow(unreachable_pub)]
#[derive(Debug, Deserialize)]
pub struct DatabaseUserResponse {}

// --- Public helpers ---

/// Context passed to [`map_atlas_error`] so the user-facing message can name
/// the affected resource.
struct AtlasErrorContext<'a> {
    /// Verb describing the failed action — used as the fallback prefix.
    /// Example: `"create database user"`.
    action: &'a str,
    /// Pre-formatted message returned when the API responds with `NOT_FOUND`.
    /// `None` falls back to a generic message built from `action`.
    not_found: Option<String>,
}

fn map_atlas_error(err: OperationError, ctx: AtlasErrorContext<'_>) -> anyhow::Error {
    match err {
        OperationError::Atlas { status, .. } if status == StatusCode::UNAUTHORIZED => {
            anyhow!("Authentication failed. Run 'atlas auth login' and try again.")
        }
        OperationError::Atlas { status, .. } if status == StatusCode::NOT_FOUND => {
            ctx.not_found.map_or_else(
                || anyhow!("Failed to {}: not found.", ctx.action),
                Error::msg,
            )
        }
        other => anyhow!("Failed to {}: {other}", ctx.action),
    }
}

/// Fetch the SRV connection string for a cluster.
pub(crate) async fn get_cluster_srv(
    client: &AtlasClient,
    project_id: &ProjectId,
    cluster: &ClusterName,
) -> Result<String> {
    let op = GetClusterOperation::builder()
        .url_parameters(
            GetClusterOperationUrlParams::builder()
                .group_id(project_id.as_str().to_owned())
                .cluster_name(cluster.as_str().to_owned())
                .build(),
        )
        .build();

    let detail: ClusterDetail = client.execute(op).await.map_err(|e| {
        map_atlas_error(
            e,
            AtlasErrorContext {
                action: "fetch cluster",
                not_found: Some(format!(
                    "Cluster '{cluster}' not found in project '{project_id}'."
                )),
            },
        )
    })?;

    Ok(detail.connection_strings.standard_srv)
}

/// Create a temporary database user with `readWriteAnyDatabase` and a
/// caller-provided expiry. Atlas removes the user automatically at
/// `delete_after_date`.
pub(crate) async fn create_temp_db_user(
    client: &AtlasClient,
    project_id: &ProjectId,
    username: &Username,
    password: &Password,
    delete_after_date: &str,
) -> Result<()> {
    let roles = TEMP_USER_ROLES
        .iter()
        .map(|(role, db)| DatabaseUserRole {
            role_name: (*role).to_string(),
            database_name: (*db).to_string(),
        })
        .collect();

    let op = CreateDatabaseUserOperation::builder()
        .url_parameters(
            CreateDatabaseUserOperationUrlParams::builder()
                .group_id(project_id.as_str().to_owned())
                .build(),
        )
        .body(CreateDatabaseUserRequest {
            database_name: AUTH_DATABASE.to_string(),
            username: username.as_str().to_owned(),
            password: password.as_str().to_owned(),
            roles,
            delete_after_date: delete_after_date.to_string(),
        })
        .build();

    let _: DatabaseUserResponse = client.execute(op).await.map_err(|e| {
        map_atlas_error(
            e,
            AtlasErrorContext {
                action: "create database user",
                not_found: None,
            },
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_detail_deserializes() {
        let json = r#"{
            "name": "my-cluster",
            "connectionStrings": {
                "standardSrv": "mongodb+srv://my-cluster.abc.mongodb.net"
            }
        }"#;
        let detail: ClusterDetail = serde_json::from_str(json).unwrap();
        assert_eq!(
            detail.connection_strings.standard_srv,
            "mongodb+srv://my-cluster.abc.mongodb.net"
        );
    }

    #[test]
    fn create_user_request_serializes_with_camel_case() {
        let req = CreateDatabaseUserRequest {
            database_name: "admin".to_string(),
            username: "atlas-sh-test".to_string(),
            password: "secret".to_string(),
            roles: vec![DatabaseUserRole {
                role_name: "readWriteAnyDatabase".to_string(),
                database_name: "admin".to_string(),
            }],
            delete_after_date: "2024-01-01T08:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("readWriteAnyDatabase"));
        assert!(json.contains("deleteAfterDate"));
        assert!(json.contains("databaseName"));
        assert!(json.contains("roleName"));
    }

    #[test]
    fn temp_user_roles_constant_is_well_formed() {
        for (role, db) in TEMP_USER_ROLES {
            assert!(!role.is_empty());
            assert!(!db.is_empty());
        }
    }
}
