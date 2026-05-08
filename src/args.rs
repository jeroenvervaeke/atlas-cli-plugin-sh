use clap::{Args, Parser, Subcommand};

const SH_LONG_ABOUT: &str = "\
Launch mongosh connected to an Atlas cluster.

PREVIEW: not production-ready. Expect breaking changes between versions.
Report issues: https://github.com/jeroenvervaeke/atlas-cli-plugin-sh/issues

Resolves the cluster SRV record via the Atlas API, provisions a short-lived
database user, caches credentials in the OS keychain, then execs mongosh.
Atlas removes the user when it expires.

Unrecognized flags are forwarded to mongosh (--eval, --quiet, --norc, --json,
etc.).";

const SH_AFTER_LONG_HELP: &str = "\
Examples:
  # Interactive shell against a cluster in the default profile
  atlas sh --cluster MyCluster

  # Run a single command and exit
  atlas sh --cluster MyCluster --eval \"show dbs\"

  # Non-default profile and explicit project ID
  atlas sh --cluster MyCluster --profile staging --project-id 5f1b...

  # Forward flags to mongosh
  atlas sh --cluster MyCluster --quiet --norc";

#[derive(Parser)]
#[command(
    version,
    about = "[preview] Launch mongosh against an Atlas cluster",
    long_about = "Launch mongosh against an Atlas cluster.\n\n\
                  PREVIEW: not production-ready. Expect breaking changes between versions.\n\n\
                  Run 'atlas sh --help' for options and examples."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: PluginSubCommands,
}

#[derive(Subcommand)]
pub enum PluginSubCommands {
    /// [preview] Launch mongosh against an Atlas cluster
    #[command(long_about = SH_LONG_ABOUT, after_long_help = SH_AFTER_LONG_HELP)]
    Sh(ShArgs),
}

#[derive(Args)]
pub struct ShArgs {
    /// Atlas cluster name (required)
    #[arg(
        long,
        visible_alias = "clusterName",
        value_name = "NAME",
        long_help = "Atlas cluster name (required).\n\n\
                     Use the name shown in the Atlas UI or 'atlas clusters list'.\n\
                     The cluster must belong to the project resolved from --project-id\n\
                     or the active Atlas CLI profile.\n\n\
                     Alias: --clusterName (matches mongodb-atlas-cli)."
    )]
    pub cluster: String,

    /// Atlas CLI profile to use
    #[arg(
        long,
        short = 'P',
        default_value = "default",
        value_name = "NAME",
        long_help = "Atlas CLI profile to use.\n\n\
                     The profile supplies API credentials, the default project ID,\n\
                     and the optional mongosh_path setting. Manage profiles with\n\
                     'atlas config' and 'atlas auth login'."
    )]
    pub profile: String,

    /// Atlas project ID (overrides profile default)
    #[arg(
        long,
        visible_alias = "projectId",
        value_name = "ID",
        long_help = "Atlas project (group) ID containing the cluster.\n\n\
                     Defaults to the project ID configured in the selected Atlas CLI\n\
                     profile. Persist a default with 'atlas config set project_id <id>'.\n\n\
                     Alias: --projectId (matches mongodb-atlas-cli)."
    )]
    pub project_id: Option<String>,

    /// Arguments forwarded to mongosh (e.g. --eval, --quiet, --norc)
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "MONGOSH_ARGS",
        long_help = "Arguments forwarded to mongosh, appended after the connection\n\
                     string and auth flags.\n\n\
                     Examples:\n  \
                       --eval \"<expression>\"   run a command and exit\n  \
                       --quiet                 suppress the startup banner\n  \
                       --norc                  skip mongoshrc.js\n  \
                       --json                  print results as JSON\n\n\
                     Run 'mongosh --help' for the full list."
    )]
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

    #[test]
    fn accepts_mongodb_atlas_cli_aliases() {
        let cli = Cli::try_parse_from([
            "atlas", "sh",
            "--clusterName", "prod",
            "-P", "staging",
            "--projectId", "abc123",
        ])
        .unwrap();
        let PluginSubCommands::Sh(args) = cli.command;
        assert_eq!(args.cluster, "prod");
        assert_eq!(args.profile, "staging");
        assert_eq!(args.project_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn cluster_name_alias_matches_cluster() {
        let cli = Cli::try_parse_from(["atlas", "sh", "--clusterName", "my-cluster"]).unwrap();
        let PluginSubCommands::Sh(args) = cli.command;
        assert_eq!(args.cluster, "my-cluster");
    }

    #[test]
    fn project_id_alias_matches_project_id() {
        let cli = Cli::try_parse_from([
            "atlas", "sh", "--cluster", "c", "--projectId", "abc123",
        ])
        .unwrap();
        let PluginSubCommands::Sh(args) = cli.command;
        assert_eq!(args.project_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn profile_short_form_matches_profile() {
        let cli = Cli::try_parse_from(["atlas", "sh", "--cluster", "c", "-P", "staging"]).unwrap();
        let PluginSubCommands::Sh(args) = cli.command;
        assert_eq!(args.profile, "staging");
    }
}
