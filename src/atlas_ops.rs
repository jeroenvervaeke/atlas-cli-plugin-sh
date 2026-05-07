use mongodb_atlas_cli::atlas::operation;
use serde::{Deserialize, Serialize};

// --- GetCluster ---

/// GET /api/atlas/v2/groups/{group_id}/clusters/{cluster_name}
#[derive(Debug)]
#[operation(method = GET, version = "2024-08-05")]
#[url("/api/atlas/v2/groups/{group_id}/clusters/{cluster_name}")]
#[response(ClusterDetail)]
struct GetClusterRequest {}

#[derive(Debug, Deserialize)]
pub struct ClusterDetail {
    #[serde(rename = "connectionStrings")]
    pub connection_strings: ConnectionStrings,
}

#[derive(Debug, Deserialize)]
pub struct ConnectionStrings {
    #[serde(rename = "standardSrv")]
    pub standard_srv: String,
}

// --- CreateDatabaseUser ---

/// POST /api/atlas/v2/groups/{group_id}/databaseUsers
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
pub struct DatabaseUserRole {
    #[serde(rename = "roleName")]
    pub role_name: String,
    #[serde(rename = "databaseName")]
    pub database_name: String,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseUserResponse {}

// --- Public helpers ---

/// Fetch the SRV connection string for a cluster.
pub async fn get_cluster_srv(
    client: &mongodb_atlas_cli::atlas::client::AtlasClient,
    group_id: &str,
    cluster_name: &str,
) -> anyhow::Result<String> {
    use http::StatusCode;
    use mongodb_atlas_cli::atlas::layer::OperationError;

    let op = GetClusterOperation::builder()
        .url_parameters(
            GetClusterOperationUrlParams::builder()
                .group_id(group_id.to_string())
                .cluster_name(cluster_name.to_string())
                .build(),
        )
        .build();

    let detail: ClusterDetail = client.execute(op).await.map_err(|e| match e {
        OperationError::Atlas { status, .. } if status == StatusCode::UNAUTHORIZED => {
            anyhow::anyhow!("Authentication failed. Run 'atlas auth login' and try again.")
        }
        OperationError::Atlas { status, .. } if status == StatusCode::NOT_FOUND => {
            anyhow::anyhow!(
                "Cluster '{}' not found in project '{}'.",
                cluster_name,
                group_id
            )
        }
        e => anyhow::anyhow!("Atlas API error: {e}"),
    })?;

    Ok(detail.connection_strings.standard_srv)
}

/// Create a temporary database user with readWriteAnyDatabase and 8h TTL.
pub async fn create_temp_db_user(
    client: &mongodb_atlas_cli::atlas::client::AtlasClient,
    group_id: &str,
    username: &str,
    password: &str,
    delete_after_date: &str,
) -> anyhow::Result<()> {
    use http::StatusCode;
    use mongodb_atlas_cli::atlas::layer::OperationError;

    let op = CreateDatabaseUserOperation::builder()
        .url_parameters(
            CreateDatabaseUserOperationUrlParams::builder()
                .group_id(group_id.to_string())
                .build(),
        )
        .body(CreateDatabaseUserRequest {
            database_name: "admin".to_string(),
            username: username.to_string(),
            password: password.to_string(),
            roles: vec![DatabaseUserRole {
                role_name: "readWriteAnyDatabase".to_string(),
                database_name: "admin".to_string(),
            }],
            delete_after_date: delete_after_date.to_string(),
        })
        .build();

    let _: DatabaseUserResponse = client.execute(op).await.map_err(|e| match e {
        OperationError::Atlas { status, .. } if status == StatusCode::UNAUTHORIZED => {
            anyhow::anyhow!("Authentication failed. Run 'atlas auth login' and try again.")
        }
        e => anyhow::anyhow!("Failed to create database user: {e}"),
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

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
    fn create_user_request_serializes() {
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
    }
}
