mod commands;
mod opts;
mod random_name;
mod spin;

use anyhow::{Error, Result};
use clap::{FromArgMatches, Parser};
use commands::{
    apps::AppsCommand,
    deploy::DeployCommand,
    link::{LinkCommand, UnlinkCommand},
    login::{LoginCommand, LogoutCommand},
    logs::LogsCommand,
    sqlite::SqliteCommand,
    variables::VariablesCommand,
};

/// Returns build information, similar to: 0.1.0 (2be4034 2022-03-31).
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("VERGEN_GIT_SHA"),
    " ",
    env!("VERGEN_GIT_COMMIT_DATE"),
    ")"
);

#[derive(Parser)]
#[clap(author, version = VERSION, about, long_about = None)]
#[clap(propagate_version = true)]
enum CloudCli {
    /// Manage applications deployed to Fermyon Cloud
    #[clap(subcommand, alias = "app")]
    Apps(AppsCommand),
    /// Package and upload an application to the Fermyon Cloud.
    Deploy(DeployCommand),
    /// Log into Fermyon Cloud
    Login(LoginCommand),
    /// Log out of Fermyon Cloud
    Logout(LogoutCommand),
    /// Fetch logs for an app from Fermyon Cloud
    Logs(LogsCommand),
    /// Manage Spin application variables
    #[clap(subcommand, alias = "vars")]
    Variables(VariablesCommand),
    /// Manage Fermyon Cloud SQLite databases
    #[clap(subcommand)]
    Sqlite(SqliteCommand),
    /// Link apps to resources
    #[clap(subcommand)]
    Link(LinkCommand),
    /// Unlink apps from resources
    #[clap(subcommand)]
    Unlink(UnlinkCommand),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::init();
    let mut app = CloudCli::clap();
    // Plugin should always be invoked from Spin so set binary name accordingly
    app.set_bin_name("spin cloud");
    let matches = app.get_matches();
    let cli = CloudCli::from_arg_matches(&matches)?;

    match cli {
        CloudCli::Apps(cmd) => cmd.run().await,
        CloudCli::Deploy(cmd) => cmd.run().await,
        CloudCli::Login(cmd) => cmd.run().await,
        CloudCli::Logout(cmd) => cmd.run().await,
        CloudCli::Logs(cmd) => cmd.run().await,
        CloudCli::Variables(cmd) => cmd.run().await,
        CloudCli::Sqlite(cmd) => cmd.run().await,
        CloudCli::Link(cmd) => cmd.run().await,
        CloudCli::Unlink(cmd) => cmd.run().await,
    }
}
