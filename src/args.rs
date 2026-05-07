use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: PluginSubCommands,
}

#[derive(Subcommand)]
pub enum PluginSubCommands {
    /// Launch mongosh connected to an Atlas cluster
    Sh(ShArgs),
}

#[derive(Args)]
pub struct ShArgs {
    /// Name of the Atlas cluster to connect to
    #[arg(long)]
    pub cluster: String,

    /// Atlas CLI profile name
    #[arg(long, default_value = "default")]
    pub profile: String,

    /// Override project ID from Atlas CLI config
    #[arg(long)]
    pub project_id: Option<String>,

    /// Extra arguments forwarded verbatim to mongosh
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub mongosh_args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_required_cluster_flag() {
        let cli = Cli::try_parse_from(["atlas", "sh", "--cluster", "my-cluster"]).unwrap();
        let PluginSubCommands::Sh(args) = cli.command;
        assert_eq!(args.cluster, "my-cluster");
        assert_eq!(args.profile, "default");
        assert!(args.project_id.is_none());
        assert!(args.mongosh_args.is_empty());
    }

    #[test]
    fn missing_cluster_fails() {
        let result = Cli::try_parse_from(["atlas", "sh"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_all_flags() {
        let cli = Cli::try_parse_from([
            "atlas", "sh",
            "--cluster", "prod",
            "--profile", "staging",
            "--project-id", "abc123",
            "--eval", "db.stats()",
        ])
        .unwrap();
        let PluginSubCommands::Sh(args) = cli.command;
        assert_eq!(args.cluster, "prod");
        assert_eq!(args.profile, "staging");
        assert_eq!(args.project_id.as_deref(), Some("abc123"));
        assert_eq!(args.mongosh_args, vec!["--eval", "db.stats()"]);
    }
}
