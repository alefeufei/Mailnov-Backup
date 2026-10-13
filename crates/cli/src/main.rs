mod commands;
#[cfg(feature = "test-secrets")]
mod file_secrets;
mod progress;

use clap::{Args, Parser, Subcommand, ValueEnum};
use mailbackup_core::TlsMode;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "mailbackup",
    version,
    about = "Archive IMAP mailboxes to local .eml files and migrate them"
)]
pub struct Cli {
    /// Configuration directory (default: %APPDATA%\mailbackup on Windows, the user config dir elsewhere)
    #[arg(
        long,
        global = true,
        env = "MAILBACKUP_CONFIG_DIR",
        value_name = "PATH"
    )]
    pub config_dir: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // parsed once at startup
pub enum Command {
    /// Manage profiles
    #[command(subcommand)]
    Profile(ProfileCmd),
    /// Download new mail from the source account into the archive
    Archive { profile: String },
    /// Copy the archive to an IMAP account or a directory of .eml files
    Export {
        profile: String,
        #[arg(long, value_enum)]
        to: ExportTo,
    },
    /// Archive, then export to the profile's target
    Migrate { profile: String },
    /// Check the archive files against the index
    Verify { profile: String },
    /// Rebuild the index from the .eml files
    Reindex { profile: String },
    /// Show message counts per folder
    Status { profile: String },
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ExportTo {
    Imap,
    Eml,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum CliTls {
    Implicit,
    Starttls,
}

impl From<CliTls> for TlsMode {
    fn from(t: CliTls) -> Self {
        match t {
            CliTls::Implicit => TlsMode::Implicit,
            CliTls::Starttls => TlsMode::StartTls,
        }
    }
}

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// Create a profile; the source password is read from the terminal (or one stdin line)
    Add(AddArgs),
    /// List profile names
    List,
    /// Show a profile (never prints secrets)
    Show { name: String },
    /// Change fields of a profile
    Edit(EditArgs),
    /// Replace a stored password (read from the terminal or one stdin line)
    SetPassword {
        name: String,
        /// Set the target account's password instead of the source's
        #[arg(long)]
        target: bool,
        /// Read the password from one line of stdin even on a terminal
        #[arg(long)]
        password_stdin: bool,
    },
    /// Delete a profile and its stored passwords (the archive directory is kept)
    Rm { name: String },
}

/// Fields shared by `profile add` and `profile edit`.
#[derive(Args, Default)]
pub struct Fields {
    /// E-mail address; the IMAP server is guessed from it
    #[arg(long, conflicts_with_all = ["host", "port", "tls"])]
    pub email: Option<String>,
    #[arg(long)]
    pub host: Option<String>,
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long, value_enum)]
    pub tls: Option<CliTls>,
    /// Login name (with --email it defaults to the address)
    #[arg(long)]
    pub user: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub archive_dir: Option<PathBuf>,
    /// Folder to archive (repeatable); default is all folders
    #[arg(long = "folder")]
    pub folders: Vec<String>,
    /// Only archive mail dated on/after YYYY-MM-DD
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub since: Option<String>,
    #[arg(long, conflicts_with = "target_eml")]
    pub target_imap_host: Option<String>,
    #[arg(long)]
    pub target_imap_port: Option<u16>,
    #[arg(long, value_enum)]
    pub target_imap_tls: Option<CliTls>,
    #[arg(long)]
    pub target_imap_user: Option<String>,
    /// Export target: a directory of .eml files
    #[arg(long, value_name = "DIR")]
    pub target_eml: Option<PathBuf>,
}

#[derive(Args)]
pub struct AddArgs {
    pub name: String,
    #[command(flatten)]
    pub fields: Fields,
    /// Read passwords from stdin lines (source first, then target if it is IMAP) even on a terminal
    #[arg(long)]
    pub password_stdin: bool,
}

#[derive(Args)]
pub struct EditArgs {
    pub name: String,
    #[command(flatten)]
    pub fields: Fields,
    /// Archive all folders again
    #[arg(long)]
    pub clear_folders: bool,
    /// Remove the date filter
    #[arg(long, conflicts_with = "since")]
    pub clear_since: bool,
    /// Remove the export target
    #[arg(long)]
    pub clear_target: bool,
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            // 0 for --help/--version, 1 for usage errors (clap's own 2 means "partial" here).
            std::process::exit(if e.use_stderr() { 1 } else { 0 });
        }
    };
    let code = match commands::run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    };
    std::process::exit(code);
}
