use clap::{Args, Parser, Subcommand};

const SH_LONG_ABOUT: &str = "\
Launch mongosh connected to an Atlas cluster — no manual connection string needed.

PREVIEW / RELEASE CANDIDATE: this plugin is not yet production-ready.
Expect breaking changes between versions. Feedback welcome at
https://github.com/jeroenvervaeke/atlas-cli-plugin-sh/issues.

The plugin looks up the cluster's SRV address through the Atlas API, provisions a
short-lived database user, caches the credentials in the OS keychain, and then
execs mongosh with the appropriate connection string and authentication flags.
The temporary user is automatically removed by Atlas after it expires.

Any flags this command does not recognize are forwarded verbatim to mongosh, so
you can use familiar mongosh options like --eval, --quiet, --norc, --json, etc.
without any special separator.";

const SH_AFTER_LONG_HELP: &str = "\
Examples:
  # Open an interactive shell against a cluster in the default profile
  atlas sh --cluster MyCluster

  # Run a single command and exit (--eval is forwarded to mongosh)
  atlas sh --cluster MyCluster --eval \"show dbs\"

  # Use a non-default Atlas CLI profile and override the project ID
  atlas sh --cluster MyCluster --profile staging --project-id 5f1b...

  # Forward additional flags to mongosh
  atlas sh --cluster MyCluster --quiet --norc";

#[derive(Parser)]
#[command(
    version,
    about = "Atlas CLI plugin that launches mongosh against an Atlas cluster [preview]",
    long_about = "Atlas CLI plugin that launches mongosh against an Atlas cluster.\n\n\
                  PREVIEW / RELEASE CANDIDATE: this plugin is not yet production-ready.\n\
                  Expect breaking changes between versions.\n\n\
                  Run 'atlas sh --help' for the full list of options and examples."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: PluginSubCommands,
}

#[derive(Subcommand)]
pub enum PluginSubCommands {
    /// Launch mongosh connected to an Atlas cluster [preview]
    #[command(long_about = SH_LONG_ABOUT, after_long_help = SH_AFTER_LONG_HELP)]
    Sh(ShArgs),
}

#[derive(Args)]
pub struct ShArgs {
    /// Name of the Atlas cluster to connect to (required)
    #[arg(
        long,
        value_name = "NAME",
        long_help = "Name of the Atlas cluster to connect to (required).\n\n\
                     This is the cluster name as shown in the Atlas UI or the output of\n\
                     'atlas clusters list'. The cluster must be in the project resolved\n\
                     from --project-id or the active Atlas CLI profile."
    )]
    pub cluster: String,

    /// Atlas CLI profile name to load credentials and project from
    #[arg(
        long,
        default_value = "default",
        value_name = "NAME",
        long_help = "Atlas CLI profile to use when calling the Atlas API.\n\n\
                     Profiles are managed by the Atlas CLI itself (see\n\
                     'atlas config' / 'atlas auth login'). The selected profile\n\
                     supplies the API credentials, default project ID, and the\n\
                     optional mongosh_path setting."
    )]
    pub profile: String,

    /// Override the project ID from the Atlas CLI profile
    #[arg(
        long,
        value_name = "ID",
        long_help = "Atlas project (group) ID to look the cluster up in.\n\n\
                     When omitted, the project ID configured in the selected\n\
                     Atlas CLI profile is used. If neither is set, the command\n\
                     fails with a configuration error.\n\n\
                     Tip: 'atlas config set project_id <id>' persists a default\n\
                     in your active profile."
    )]
    pub project_id: Option<String>,

    /// Additional arguments forwarded verbatim to mongosh (e.g. --eval, --quiet, --norc)
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "MONGOSH_ARGS",
        long_help = "Any unrecognized arguments are forwarded verbatim to mongosh,\n\
                     appended after the connection string and authentication flags\n\
                     that this plugin supplies.\n\n\
                     Common examples:\n  \
                       --eval \"<expression>\"   run a single command and exit\n  \
                       --quiet                  suppress the startup banner\n  \
                       --norc                   skip the user's mongoshrc.js\n  \
                       --json                   print results as JSON\n\n\
                     See 'mongosh --help' for the full list."
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
}
