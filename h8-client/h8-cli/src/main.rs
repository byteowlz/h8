//! h8 CLI - Exchange Web Services client.

use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::Command as ProcCommand;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{
    DateTime, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc,
};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use env_logger::fmt::WriteStyle;
use h8_core::id::WordLists;
use h8_core::maildir::{FOLDER_DRAFTS, FOLDER_TRASH, MessageFlags};
use h8_core::{
    AppConfig, AppPaths, ComposeBuilder, ComposeDocument, Database, IdGenerator, Maildir,
    ServiceClient,
};

use log::{LevelFilter, debug};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const APP_NAME: &str = env!("CARGO_PKG_NAME");

fn main() {
    if let Err(err) = try_main() {
        let _ = writeln!(io::stderr(), "{err:?}");
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    let cli = Cli::parse();
    let ctx = RuntimeContext::new(cli.common.clone())?;
    ctx.init_logging()?;
    debug!("config loaded from {}", ctx.paths.global_config.display());

    match cli.command {
        Command::Calendar { command } => handle_calendar(&ctx, command),
        Command::Mail { command } => handle_mail(&ctx, command),
        Command::Agenda(args) => handle_agenda(&ctx, args),
        Command::Contacts { command } => handle_contacts(&ctx, command),
        Command::Free(cmd) => handle_free(&ctx, cmd),
        Command::Config { command } => handle_config(&ctx, command),
        Command::Init(cmd) => handle_init(&ctx, cmd),
        Command::Completions { shell } => handle_completions(shell),
        Command::Service { command } => handle_service(&ctx, command),
    }
}

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Rust CLI for EWS backed by the Python service.",
    propagate_version = true
)]
struct Cli {
    #[command(flatten)]
    common: CommonOpts,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Args)]
struct CommonOpts {
    #[arg(long, value_name = "PATH", global = true)]
    config: Option<PathBuf>,
    #[arg(short, long, action = clap::ArgAction::SetTrue, global = true)]
    quiet: bool,
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    #[arg(long, global = true)]
    debug: bool,
    #[arg(long, global = true)]
    trace: bool,
    #[arg(long, global = true, conflicts_with = "yaml")]
    json: bool,
    #[arg(long, global = true)]
    yaml: bool,
    #[arg(long = "no-color", global = true, conflicts_with = "color")]
    no_color: bool,
    #[arg(long, value_enum, default_value_t = ColorOption::Auto, global = true)]
    color: ColorOption,
    #[arg(long = "dry-run", global = true)]
    dry_run: bool,
    #[arg(short = 'y', long = "yes", alias = "force", global = true)]
    assume_yes: bool,
    #[arg(long = "timeout", value_name = "SECONDS", global = true)]
    timeout: Option<u64>,
    #[arg(long = "no-progress", global = true)]
    no_progress: bool,
    #[arg(long = "diagnostics", global = true)]
    diagnostics: bool,
    #[arg(short = 'a', long = "account", global = true)]
    account: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ColorOption {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(alias = "cal")]
    Calendar {
        #[command(subcommand)]
        command: CalendarCommand,
    },
    Mail {
        #[command(subcommand)]
        command: MailCommand,
    },
    /// Today's agenda (calendar)
    Agenda(AgendaArgs),
    Contacts {
        #[command(subcommand)]
        command: ContactsCommand,
    },
    Free(FreeCommand),
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Init(InitCommand),
    /// Manage the Python EWS service
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
enum CalendarCommand {
    #[command(alias = "ls")]
    List(CalendarListArgs),
    Create(CalendarCreateArgs),
    Delete(CalendarDeleteArgs),
}

#[derive(Debug, Args)]
struct CalendarListArgs {
    #[arg(short = 'd', long, default_value_t = 7)]
    days: i64,
    #[arg(long = "from")]
    from_date: Option<String>,
    #[arg(long = "to")]
    to_date: Option<String>,
}

#[derive(Debug, Args)]
struct CalendarCreateArgs {
    #[arg(long)]
    file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct CalendarDeleteArgs {
    #[arg(long)]
    id: String,
    #[arg(long = "changekey")]
    change_key: Option<String>,
}

#[derive(Debug, Subcommand)]
enum MailCommand {
    /// List messages in a folder
    #[command(alias = "ls")]
    List(MailListArgs),
    /// Get a message by ID
    Get(MailGetArgs),
    /// Read a message (view in pager)
    Read(MailReadArgs),
    /// Fetch messages from server to local storage
    Fetch(MailFetchArgs),
    /// Send an email
    Send(MailSendArgs),
    /// Compose a new email
    Compose(MailComposeArgs),
    /// Reply to a message
    Reply(MailReplyArgs),
    /// Forward a message
    Forward(MailForwardArgs),
    /// Move a message to another folder
    #[command(alias = "mv")]
    Move(MailMoveArgs),
    /// Delete a message (move to trash)
    #[command(alias = "rm")]
    Delete(MailDeleteArgs),
    /// Mark a message (read/unread/flagged)
    Mark(MailMarkArgs),
    /// List drafts
    Drafts(MailDraftsArgs),
    /// Edit an existing draft
    Edit(MailEditArgs),
    /// Sync messages with server
    Sync(MailSyncArgs),
    /// List or download attachments
    #[command(alias = "att")]
    Attachments(MailAttachmentsArgs),
}

#[derive(Debug, Args)]
struct MailListArgs {
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    #[arg(short = 'l', long, default_value_t = 20)]
    limit: usize,
    #[arg(short = 'u', long)]
    unread: bool,
}

#[derive(Debug, Args)]
struct MailGetArgs {
    #[arg(long)]
    id: String,
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
}

#[derive(Debug, Args)]
struct AgendaArgs {
    #[arg(short = 'a', long = "account")]
    account: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgendaItem {
    subject: Option<String>,
    start: Option<String>,
    end: Option<String>,
    location: Option<String>,
    is_all_day: Option<bool>,
}

#[derive(Debug, Args)]
struct MailFetchArgs {
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    #[arg(short = 'o', long)]
    output: PathBuf,
    #[arg(long, value_enum, default_value_t = FetchFormat::Maildir)]
    format: FetchFormat,
    #[arg(short = 'l', long)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, ValueEnum)]
enum FetchFormat {
    Maildir,
    Mbox,
}

impl From<FetchFormat> for h8_core::types::FetchFormat {
    fn from(f: FetchFormat) -> Self {
        match f {
            FetchFormat::Maildir => h8_core::types::FetchFormat::Maildir,
            FetchFormat::Mbox => h8_core::types::FetchFormat::Mbox,
        }
    }
}

#[derive(Debug, Args)]
struct MailSendArgs {
    /// Draft ID to send (if not provided, reads from file/stdin)
    id: Option<String>,
    /// Read email from file instead of stdin
    #[arg(long)]
    file: Option<PathBuf>,
    /// Send all drafts
    #[arg(long)]
    all: bool,
}

#[derive(Debug, Args)]
struct MailReadArgs {
    /// Message ID (e.g., 'cold-lamp')
    id: String,
    /// Folder to read from
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    /// Show raw RFC822 format
    #[arg(long)]
    raw: bool,
}

#[derive(Debug, Args)]
struct MailComposeArgs {
    /// Open editor immediately (default behavior)
    #[arg(long)]
    no_edit: bool,
}

#[derive(Debug, Args)]
struct MailReplyArgs {
    /// Message ID to reply to
    id: String,
    /// Folder containing the message
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    /// Reply to all recipients
    #[arg(long, short = 'a')]
    all: bool,
}

#[derive(Debug, Args)]
struct MailForwardArgs {
    /// Message ID to forward
    id: String,
    /// Folder containing the message
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
}

#[derive(Debug, Args)]
struct MailMoveArgs {
    /// Message ID to move
    id: String,
    /// Destination folder
    dest: String,
    /// Source folder
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
}

#[derive(Debug, Args)]
struct MailDeleteArgs {
    /// Message ID to delete
    id: String,
    /// Folder containing the message
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    /// Permanently delete (skip trash)
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct MailMarkArgs {
    /// Message ID to mark
    id: String,
    /// Folder containing the message
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    /// Mark as read
    #[arg(long, conflicts_with = "unread")]
    read: bool,
    /// Mark as unread
    #[arg(long, conflicts_with = "read")]
    unread: bool,
    /// Mark as flagged/starred
    #[arg(long, conflicts_with = "unflag")]
    flag: bool,
    /// Remove flagged/starred
    #[arg(long, conflicts_with = "flag")]
    unflag: bool,
}

#[derive(Debug, Args)]
struct MailDraftsArgs {
    /// Maximum number of drafts to list
    #[arg(short = 'l', long, default_value_t = 20)]
    limit: usize,
}

#[derive(Debug, Args)]
struct MailEditArgs {
    /// Draft ID to edit
    id: String,
}

#[derive(Debug, Args)]
struct MailSyncArgs {
    /// Folder to sync (default: all configured folders)
    folder: Option<String>,
    /// Force full re-sync (ignore sync tokens)
    #[arg(long)]
    full: bool,
    /// Only sync emails received in the last N days
    #[arg(short = 'l', long = "limit", value_name = "DAYS")]
    limit_days: Option<u32>,
}

#[derive(Debug, Args)]
struct MailAttachmentsArgs {
    /// Message ID
    id: String,
    /// Folder containing the message
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    /// Download attachment by index
    #[arg(short = 'd', long)]
    download: Option<usize>,
    /// Output path (directory or file)
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ContactsCommand {
    #[command(alias = "ls")]
    List(ContactsListArgs),
    Get(ContactsGetArgs),
    Create(ContactsCreateArgs),
    Delete(ContactsDeleteArgs),
}

#[derive(Debug, Args)]
struct ContactsListArgs {
    #[arg(short = 'l', long, default_value_t = 100)]
    limit: usize,
    #[arg(short = 's', long)]
    search: Option<String>,
}

#[derive(Debug, Args)]
struct ContactsGetArgs {
    #[arg(long)]
    id: String,
}

#[derive(Debug, Args)]
struct ContactsCreateArgs {
    #[arg(long)]
    file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ContactsDeleteArgs {
    #[arg(long)]
    id: String,
}

#[derive(Debug, Args)]
struct FreeCommand {
    #[arg(short = 'w', long, default_value_t = 1)]
    weeks: u8,
    #[arg(short = 'd', long, default_value_t = 30)]
    duration: u32,
    #[arg(short = 'l', long)]
    limit: Option<usize>,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Show,
    Path,
    Reset,
}

#[derive(Debug, Clone, Args)]
struct InitCommand {
    #[arg(long = "force")]
    force: bool,
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Start the Python service (background)
    Start,
    /// Stop the Python service
    Stop,
    /// Restart the Python service
    Restart,
    /// Show service status
    Status,
}

#[derive(Debug, Clone)]
struct RuntimeContext {
    common: CommonOpts,
    paths: AppPaths,
    config: AppConfig,
}

impl RuntimeContext {
    fn new(common: CommonOpts) -> Result<Self> {
        let paths = AppPaths::discover(common.config.clone()).map_err(|e| anyhow!("{e}"))?;
        AppConfig::ensure_default(&paths.global_config).map_err(|e| anyhow!("{e}"))?;
        let config =
            AppConfig::load(&paths, common.account.as_deref()).map_err(|e| anyhow!("{e}"))?;
        Ok(Self {
            common,
            paths,
            config,
        })
    }

    fn init_logging(&self) -> Result<()> {
        if self.common.quiet {
            log::set_max_level(LevelFilter::Off);
            return Ok(());
        }

        let mut builder =
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));

        builder.filter_level(self.effective_log_level());

        let force_color = matches!(self.common.color, ColorOption::Always)
            || env::var_os("FORCE_COLOR").is_some();
        let disable_color = self.common.no_color
            || matches!(self.common.color, ColorOption::Never)
            || env::var_os("NO_COLOR").is_some();

        if disable_color {
            builder.write_style(WriteStyle::Never);
        } else if force_color {
            builder.write_style(WriteStyle::Always);
        } else {
            builder.write_style(WriteStyle::Auto);
        }

        if self.common.diagnostics {
            builder.format_timestamp_millis();
            builder.format_module_path(true);
            builder.format_target(true);
        }

        builder.try_init().or_else(|err| {
            if self.common.verbose > 0 {
                eprintln!("logger already initialized: {err}");
            }
            Ok(())
        })
    }

    fn effective_log_level(&self) -> LevelFilter {
        if self.common.trace {
            LevelFilter::Trace
        } else if self.common.debug {
            LevelFilter::Debug
        } else {
            match self.common.verbose {
                0 => LevelFilter::Info,
                1 => LevelFilter::Debug,
                _ => LevelFilter::Trace,
            }
        }
    }

    fn service_client(&self) -> Result<ServiceClient> {
        let timeout = self.common.timeout.map(Duration::from_secs);
        ServiceClient::new(&self.config.service_url, timeout).map_err(|e| anyhow!("{e}"))
    }
}

fn handle_calendar(ctx: &RuntimeContext, cmd: CalendarCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ctx.service_client()?;
    match cmd {
        CalendarCommand::List(args) => {
            let events = client
                .calendar_list(
                    &account,
                    args.days,
                    args.from_date.as_deref(),
                    args.to_date.as_deref(),
                )
                .map_err(|e| anyhow!("{e}"))?;
            emit_output(&ctx.common, &events)?;
        }
        CalendarCommand::Create(args) => {
            let payload = read_json_payload(args.file.as_ref())?;
            let event = client
                .calendar_create(&account, payload)
                .map_err(|e| anyhow!("{e}"))?;
            emit_output(&ctx.common, &event)?;
        }
        CalendarCommand::Delete(args) => {
            let result = client
                .calendar_delete(&account, &args.id, args.change_key.as_deref())
                .map_err(|e| anyhow!("{e}"))?;
            emit_output(&ctx.common, &result)?;
        }
    }
    Ok(())
}

fn handle_mail(ctx: &RuntimeContext, cmd: MailCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ctx.service_client()?;

    match cmd {
        MailCommand::List(args) => handle_mail_list(ctx, &client, &account, args),
        MailCommand::Get(args) => handle_mail_get(ctx, &client, &account, args),
        MailCommand::Read(args) => handle_mail_read(ctx, &account, args),
        MailCommand::Fetch(args) => handle_mail_fetch(ctx, &client, &account, args),
        MailCommand::Send(args) => handle_mail_send(ctx, &client, &account, args),
        MailCommand::Compose(args) => handle_mail_compose(ctx, &account, args),
        MailCommand::Reply(args) => handle_mail_reply(ctx, &client, &account, args),
        MailCommand::Forward(args) => handle_mail_forward(ctx, &client, &account, args),
        MailCommand::Move(args) => handle_mail_move(ctx, &account, args),
        MailCommand::Delete(args) => handle_mail_delete(ctx, &account, args),
        MailCommand::Mark(args) => handle_mail_mark(ctx, &account, args),
        MailCommand::Drafts(args) => handle_mail_drafts(ctx, &account, args),
        MailCommand::Edit(args) => handle_mail_edit(ctx, &account, args),
        MailCommand::Sync(args) => handle_mail_sync(ctx, &client, &account, args),
        MailCommand::Attachments(args) => handle_mail_attachments(ctx, &client, &account, args),
    }
}

fn handle_mail_list(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailListArgs,
) -> Result<()> {
    // Try to list from local Maildir first, fall back to server
    let mail_dir = get_mail_dir(ctx, account)?;

    if mail_dir.base_path().exists() {
        // List from local storage
        let messages = mail_dir.list(&args.folder).map_err(|e| anyhow!("{e}"))?;

        // Load database to get human-readable IDs
        let db_path = ctx.paths.sync_db_path(account);
        let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;

        let mut output: Vec<serde_json::Value> = Vec::new();
        for msg in messages.iter().take(args.limit) {
            // Try to get metadata from database
            let local_msg = db.get_message(&msg.id).map_err(|e| anyhow!("{e}"))?;

            let subject = local_msg
                .as_ref()
                .and_then(|m| m.subject.clone())
                .unwrap_or_else(|| "(no subject)".to_string());
            let from = local_msg
                .as_ref()
                .and_then(|m| m.from_addr.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let date = local_msg
                .as_ref()
                .and_then(|m| m.received_at.clone())
                .unwrap_or_default();

            // Filter unread if requested
            if args.unread && msg.flags.seen {
                continue;
            }

            output.push(serde_json::json!({
                "id": msg.id,
                "subject": subject,
                "from": from,
                "date": date,
                "is_read": msg.flags.seen,
                "is_flagged": msg.flags.flagged,
                "folder": msg.folder,
            }));
        }

        emit_output(&ctx.common, &output)?;
    } else {
        // Fall back to server
        let messages = client
            .mail_list(account, &args.folder, args.limit, args.unread)
            .map_err(|e| anyhow!("{e}"))?;
        emit_output(&ctx.common, &messages)?;
    }

    Ok(())
}

fn handle_mail_get(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailGetArgs,
) -> Result<()> {
    // Try to resolve human-readable ID to remote ID
    let db_path = ctx.paths.sync_db_path(account);
    let remote_id = if db_path.exists() {
        let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
        let id_gen = IdGenerator::new(&db);
        id_gen
            .resolve(&args.id)
            .map_err(|e| anyhow!("{e}"))?
            .unwrap_or_else(|| args.id.clone())
    } else {
        args.id.clone()
    };

    let message = client
        .mail_get(account, &args.folder, &remote_id)
        .map_err(|e| anyhow!("{e}"))?;
    emit_output(&ctx.common, &message)?;
    Ok(())
}

fn handle_mail_read(ctx: &RuntimeContext, account: &str, args: MailReadArgs) -> Result<()> {
    let mail_dir = get_mail_dir(ctx, account)?;

    // Get the message
    let msg = mail_dir
        .get(&args.folder, &args.id)
        .map_err(|e| anyhow!("{e}"))?
        .ok_or_else(|| anyhow!("message not found: {}", args.id))?;

    let content = msg.read_content().map_err(|e| anyhow!("{e}"))?;

    if args.raw || !io::stdout().is_terminal() {
        println!("{}", content);
    } else {
        // Use pager
        let pager = ctx.config.mail.pager.clone();
        let pager_parts: Vec<&str> = pager.split_whitespace().collect();
        let (pager_cmd, pager_args) = pager_parts
            .split_first()
            .ok_or_else(|| anyhow!("invalid pager command"))?;

        let mut child = ProcCommand::new(pager_cmd)
            .args(pager_args.iter())
            .stdin(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting pager: {}", pager))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(content.as_bytes())?;
        }
        child.wait()?;
    }

    // Mark as read (unless already read)
    if !msg.flags.seen {
        let mut new_flags = msg.flags.clone();
        new_flags.mark_read();
        mail_dir
            .update_flags(&args.folder, &args.id, &new_flags)
            .map_err(|e| anyhow!("{e}"))?;
    }

    Ok(())
}

fn handle_mail_fetch(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailFetchArgs,
) -> Result<()> {
    let result = client
        .mail_fetch(
            account,
            &args.folder,
            &args.output,
            args.format.into(),
            args.limit,
        )
        .map_err(|e| anyhow!("{e}"))?;
    emit_output(&ctx.common, &result)?;
    Ok(())
}

fn handle_mail_send(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailSendArgs,
) -> Result<()> {
    if args.all {
        // Send all drafts
        let mail_dir = get_mail_dir(ctx, account)?;
        let drafts = mail_dir.list(FOLDER_DRAFTS).map_err(|e| anyhow!("{e}"))?;

        for draft in drafts {
            send_draft(ctx, client, account, &mail_dir, &draft.id)?;
        }
        return Ok(());
    }

    if let Some(id) = args.id {
        // Send specific draft
        let mail_dir = get_mail_dir(ctx, account)?;
        send_draft(ctx, client, account, &mail_dir, &id)?;
    } else {
        // Read from file/stdin
        let payload = read_json_payload(args.file.as_ref())?;
        let result = client
            .mail_send(account, payload)
            .map_err(|e| anyhow!("{e}"))?;
        emit_output(&ctx.common, &result)?;
    }

    Ok(())
}

fn send_draft(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    mail_dir: &Maildir,
    draft_id: &str,
) -> Result<()> {
    // Load draft from local storage
    let draft = mail_dir
        .get(FOLDER_DRAFTS, draft_id)
        .map_err(|e| anyhow!("{e}"))?
        .ok_or_else(|| anyhow!("draft not found: {}", draft_id))?;

    let content = draft.read_content().map_err(|e| anyhow!("{e}"))?;
    let doc = ComposeDocument::parse(&content).map_err(|e| anyhow!("{e}"))?;

    // Validate before sending
    doc.validate().map_err(|e| anyhow!("{e}"))?;

    // Build send payload
    let payload = serde_json::json!({
        "to": doc.to,
        "cc": doc.cc,
        "bcc": doc.bcc,
        "subject": doc.subject,
        "body": doc.body,
        "html": false,
    });

    // Send via service
    let result = client
        .mail_send(account, payload)
        .map_err(|e| anyhow!("{e}"))?;

    // Delete local draft on success
    mail_dir
        .delete(FOLDER_DRAFTS, draft_id)
        .map_err(|e| anyhow!("{e}"))?;

    println!("Sent: {}", draft_id);
    emit_output(&ctx.common, &result)?;

    Ok(())
}

fn handle_mail_compose(ctx: &RuntimeContext, account: &str, args: MailComposeArgs) -> Result<()> {
    let doc = ComposeBuilder::new().subject("").body("").build();

    // Add signature if configured
    let mut doc = doc;
    if ctx.config.mail.compose.include_signature && !ctx.config.mail.signature.is_empty() {
        doc.add_signature(&ctx.config.mail.signature);
    }

    open_editor_and_save_draft(ctx, account, doc, !args.no_edit)
}

fn handle_mail_reply(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailReplyArgs,
) -> Result<()> {
    // Get original message
    let message = client
        .mail_get(account, &args.folder, &args.id)
        .map_err(|e| anyhow!("{e}"))?;

    let original_from = message.get("from").and_then(|v| v.as_str()).unwrap_or("");
    let original_subject = message
        .get("subject")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let original_body = message.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let original_message_id = message.get("message_id").and_then(|v| v.as_str());
    let original_references = message.get("references").and_then(|v| v.as_str());

    let doc = if args.all {
        let original_to: Vec<String> = message
            .get("to")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let original_cc: Vec<String> = message
            .get("cc")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        ComposeDocument::reply_all(
            original_from,
            &original_to,
            &original_cc,
            original_subject,
            original_message_id,
            original_references,
            original_body,
            account,
            &ctx.config.mail.compose,
        )
    } else {
        ComposeDocument::reply(
            original_from,
            original_subject,
            original_message_id,
            original_references,
            original_body,
            &ctx.config.mail.compose,
        )
    };

    let mut doc = doc;
    if ctx.config.mail.compose.include_signature && !ctx.config.mail.signature.is_empty() {
        doc.add_signature(&ctx.config.mail.signature);
    }

    open_editor_and_save_draft(ctx, account, doc, true)
}

fn handle_mail_forward(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailForwardArgs,
) -> Result<()> {
    // Get original message
    let message = client
        .mail_get(account, &args.folder, &args.id)
        .map_err(|e| anyhow!("{e}"))?;

    let original_from = message.get("from").and_then(|v| v.as_str()).unwrap_or("");
    let original_to: Vec<String> = message
        .get("to")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let original_subject = message
        .get("subject")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let original_body = message.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let original_date = message.get("datetime_received").and_then(|v| v.as_str());

    let doc = ComposeDocument::forward(
        original_from,
        &original_to,
        original_subject,
        original_date,
        original_body,
        &ctx.config.mail.compose,
    );

    let mut doc = doc;
    if ctx.config.mail.compose.include_signature && !ctx.config.mail.signature.is_empty() {
        doc.add_signature(&ctx.config.mail.signature);
    }

    open_editor_and_save_draft(ctx, account, doc, true)
}

fn handle_mail_move(ctx: &RuntimeContext, account: &str, args: MailMoveArgs) -> Result<()> {
    let mail_dir = get_mail_dir(ctx, account)?;

    mail_dir
        .move_to(&args.folder, &args.id, &args.dest)
        .map_err(|e| anyhow!("{e}"))?
        .ok_or_else(|| anyhow!("message not found: {}", args.id))?;

    println!("Moved {} to {}", args.id, args.dest);
    Ok(())
}

fn handle_mail_delete(ctx: &RuntimeContext, account: &str, args: MailDeleteArgs) -> Result<()> {
    let mail_dir = get_mail_dir(ctx, account)?;

    if args.force {
        // Permanently delete
        let deleted = mail_dir
            .delete(&args.folder, &args.id)
            .map_err(|e| anyhow!("{e}"))?;
        if deleted {
            println!("Deleted {}", args.id);
        } else {
            return Err(anyhow!("message not found: {}", args.id));
        }
    } else {
        // Move to trash
        mail_dir
            .move_to(&args.folder, &args.id, FOLDER_TRASH)
            .map_err(|e| anyhow!("{e}"))?
            .ok_or_else(|| anyhow!("message not found: {}", args.id))?;
        println!("Moved {} to trash", args.id);
    }

    Ok(())
}

fn handle_mail_mark(ctx: &RuntimeContext, account: &str, args: MailMarkArgs) -> Result<()> {
    let mail_dir = get_mail_dir(ctx, account)?;

    let msg = mail_dir
        .get(&args.folder, &args.id)
        .map_err(|e| anyhow!("{e}"))?
        .ok_or_else(|| anyhow!("message not found: {}", args.id))?;

    let mut flags = msg.flags.clone();

    if args.read {
        flags.seen = true;
    }
    if args.unread {
        flags.seen = false;
    }
    if args.flag {
        flags.flagged = true;
    }
    if args.unflag {
        flags.flagged = false;
    }

    mail_dir
        .update_flags(&args.folder, &args.id, &flags)
        .map_err(|e| anyhow!("{e}"))?;

    println!("Updated flags for {}", args.id);
    Ok(())
}

fn handle_mail_drafts(ctx: &RuntimeContext, account: &str, args: MailDraftsArgs) -> Result<()> {
    let mail_dir = get_mail_dir(ctx, account)?;
    let drafts = mail_dir.list(FOLDER_DRAFTS).map_err(|e| anyhow!("{e}"))?;

    let mut output: Vec<serde_json::Value> = Vec::new();
    for draft in drafts.iter().take(args.limit) {
        // Try to parse the draft to extract headers
        let content = draft.read_content().unwrap_or_default();
        let doc = ComposeDocument::parse(&content).ok();

        output.push(serde_json::json!({
            "id": draft.id,
            "subject": doc.as_ref().map(|d| d.subject.clone()).unwrap_or_default(),
            "to": doc.as_ref().map(|d| d.to.clone()).unwrap_or_default(),
        }));
    }

    emit_output(&ctx.common, &output)?;
    Ok(())
}

fn handle_mail_edit(ctx: &RuntimeContext, account: &str, args: MailEditArgs) -> Result<()> {
    let mail_dir = get_mail_dir(ctx, account)?;

    // Load existing draft
    let draft = mail_dir
        .get(FOLDER_DRAFTS, &args.id)
        .map_err(|e| anyhow!("{e}"))?
        .ok_or_else(|| anyhow!("draft not found: {}", args.id))?;

    let content = draft.read_content().map_err(|e| anyhow!("{e}"))?;
    let doc = ComposeDocument::parse(&content).map_err(|e| anyhow!("{e}"))?;

    // Delete old draft before creating new one
    mail_dir
        .delete(FOLDER_DRAFTS, &args.id)
        .map_err(|e| anyhow!("{e}"))?;

    open_editor_and_save_draft(ctx, account, doc, true)
}

fn handle_mail_sync(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailSyncArgs,
) -> Result<()> {
    let mail_dir = get_mail_dir(ctx, account)?;
    mail_dir.init().map_err(|e| anyhow!("{e}"))?;

    let db_path = ctx.paths.sync_db_path(account);
    let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
    let limit_days = args.limit_days;
    let cutoff_time = limit_days.map(|days| Utc::now() - ChronoDuration::days(days as i64));

    // Initialize ID pool if empty
    let id_gen = IdGenerator::new(&db);
    let stats = id_gen.stats().map_err(|e| anyhow!("{e}"))?;
    if stats.total() == 0 {
        let words = WordLists::embedded();
        id_gen.init_pool(&words).map_err(|e| anyhow!("{e}"))?;
        println!("Initialized ID pool");
    }

    // Determine folders to sync
    let folders: Vec<String> = if let Some(folder) = args.folder {
        vec![folder]
    } else {
        ctx.config.mail.sync_folders.clone()
    };

    for folder in &folders {
        println!("Syncing {}...", folder);

        // Fetch from server to local
        let messages = client
            .mail_list(account, folder, 100, false)
            .map_err(|e| anyhow!("{e}"))?;

        let messages_arr = messages
            .as_array()
            .ok_or_else(|| anyhow!("expected array from server"))?;

        // First pass: collect messages to sync (filter by cutoff, skip existing)
        let mut to_sync: Vec<(&str, &Value)> = Vec::new();
        for msg_val in messages_arr {
            if let Some(ref cutoff) = cutoff_time {
                let email_ts = msg_val
                    .get("datetime_received")
                    .and_then(|v| v.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc));
                if let Some(ts) = email_ts {
                    if ts < *cutoff {
                        continue;
                    }
                }
            }

            let remote_id = msg_val
                .get("item_id")
                .or_else(|| msg_val.get("id"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("missing item_id"))?;

            // Check if we already have this message
            if db
                .get_message_by_remote_id(remote_id)
                .map_err(|e| anyhow!("{e}"))?
                .is_some()
            {
                continue;
            }

            to_sync.push((remote_id, msg_val));
        }

        let total = to_sync.len();
        if total == 0 {
            println!("  No new messages");
            continue;
        }

        println!("  Fetching {} new messages...", total);

        let mut synced = 0;
        let mut failed = 0;
        for (i, (remote_id, msg_val)) in to_sync.into_iter().enumerate() {
            // Progress indicator
            print!("\r  [{}/{}] Syncing...    ", i + 1, total);
            let _ = io::stdout().flush();

            // Allocate human-readable ID
            let local_id = id_gen.allocate(remote_id).map_err(|e| anyhow!("{e}"))?;

            // Fetch full message content (needed for body)
            let full_msg = match client.mail_get(account, folder, remote_id) {
                Ok(msg) => msg,
                Err(e) => {
                    log::warn!("Failed to fetch message {}: {}", remote_id, e);
                    failed += 1;
                    continue;
                }
            };

            // Build email content - use full_msg for body, msg_val for metadata
            let subject = msg_val
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("(no subject)");
            let from = msg_val
                .get("from")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let body = full_msg.get("body").and_then(|v| v.as_str()).unwrap_or("");
            let date = msg_val
                .get("datetime_received")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let content = format!(
                "From: {}\r\nSubject: {}\r\nDate: {}\r\n\r\n{}",
                from, subject, date, body
            );

            // Determine flags from list response (faster than full_msg)
            let is_read = msg_val
                .get("is_read")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mut flags = MessageFlags::default();
            if is_read {
                flags.seen = true;
            }

            // Store in Maildir
            mail_dir
                .store_with_id(folder, content.as_bytes(), &flags, &local_id)
                .map_err(|e| anyhow!("{e}"))?;

            // Check for attachments from list response
            let has_attachments = msg_val
                .get("has_attachments")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Store in database
            let msg_sync = h8_core::types::MessageSync {
                local_id: local_id.clone(),
                remote_id: remote_id.to_string(),
                change_key: msg_val
                    .get("changekey")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                folder: folder.clone(),
                subject: Some(subject.to_string()),
                from_addr: Some(from.to_string()),
                received_at: Some(date.to_string()),
                is_read,
                is_draft: folder == FOLDER_DRAFTS,
                has_attachments,
                synced_at: Some(chrono::Utc::now().to_rfc3339()),
                local_hash: None,
            };
            db.upsert_message(&msg_sync).map_err(|e| anyhow!("{e}"))?;

            synced += 1;
        }

        if failed > 0 {
            println!("\r  Synced {} messages ({} failed)     ", synced, failed);
        } else {
            println!("\r  Synced {} new messages              ", synced);
        }
    }

    println!("Sync complete");
    Ok(())
}

fn handle_mail_attachments(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailAttachmentsArgs,
) -> Result<()> {
    // Resolve human-readable ID to remote ID if needed
    let db_path = ctx.paths.sync_db_path(account);
    let remote_id = if db_path.exists() {
        let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
        let id_gen = IdGenerator::new(&db);
        id_gen
            .resolve(&args.id)
            .map_err(|e| anyhow!("{e}"))?
            .unwrap_or_else(|| args.id.clone())
    } else {
        args.id.clone()
    };

    if let Some(index) = args.download {
        // Download specific attachment
        let output_path = args.output.unwrap_or_else(|| PathBuf::from("."));
        let result = client
            .mail_attachment_download(account, &args.folder, &remote_id, index, &output_path)
            .map_err(|e| anyhow!("{e}"))?;

        if let Some(path) = result.get("path").and_then(|v| v.as_str()) {
            println!("Downloaded: {}", path);
        }
        emit_output(&ctx.common, &result)?;
    } else {
        // List attachments
        let attachments = client
            .mail_attachments_list(account, &args.folder, &remote_id)
            .map_err(|e| anyhow!("{e}"))?;

        if let Some(arr) = attachments.as_array() {
            if arr.is_empty() {
                println!("No attachments");
                return Ok(());
            }

            for att in arr {
                let idx = att.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                let name = att
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unnamed");
                let size = att.get("size").and_then(|v| v.as_u64());
                let ctype = att
                    .get("content_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                if let Some(s) = size {
                    println!("[{}] {} ({} bytes, {})", idx, name, s, ctype);
                } else {
                    println!("[{}] {} ({})", idx, name, ctype);
                }
            }
        } else {
            emit_output(&ctx.common, &attachments)?;
        }
    }

    Ok(())
}

fn get_mail_dir(ctx: &RuntimeContext, account: &str) -> Result<Maildir> {
    let mail_path = if let Some(ref data_dir) = ctx.config.mail.data_dir {
        h8_core::paths::expand_str_path(data_dir)
            .map_err(|e| anyhow!("{e}"))?
            .join(account)
    } else {
        ctx.paths.mail_dir(account)
    };

    Maildir::new(mail_path, account).map_err(|e| anyhow!("{e}"))
}

fn open_editor_and_save_draft(
    ctx: &RuntimeContext,
    account: &str,
    doc: ComposeDocument,
    open_editor: bool,
) -> Result<()> {
    let content = doc.to_string().map_err(|e| anyhow!("{e}"))?;

    // Create temp file
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("h8-compose-{}.eml", std::process::id()));
    fs::write(&temp_path, &content)?;

    if open_editor {
        // Get editor command
        let editor = ctx
            .config
            .mail
            .editor
            .clone()
            .or_else(|| env::var("EDITOR").ok())
            .unwrap_or_else(|| "vi".to_string());

        // Open editor
        let status = ProcCommand::new(&editor)
            .arg(&temp_path)
            .status()
            .with_context(|| format!("starting editor: {}", editor))?;

        if !status.success() {
            let _ = fs::remove_file(&temp_path);
            return Err(anyhow!("editor exited with non-zero status"));
        }
    }

    // Read edited content
    let edited_content = fs::read_to_string(&temp_path)?;
    let _ = fs::remove_file(&temp_path);

    // Parse to validate
    let _edited_doc = ComposeDocument::parse(&edited_content).map_err(|e| anyhow!("{e}"))?;

    // Save as draft
    let mail_dir = get_mail_dir(ctx, account)?;
    mail_dir.init().map_err(|e| anyhow!("{e}"))?;

    let flags = MessageFlags {
        draft: true,
        ..Default::default()
    };

    let draft = mail_dir
        .store(FOLDER_DRAFTS, edited_content.as_bytes(), &flags)
        .map_err(|e| anyhow!("{e}"))?;

    println!("Draft saved: {}", draft.id);

    Ok(())
}

fn handle_contacts(ctx: &RuntimeContext, cmd: ContactsCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ctx.service_client()?;
    match cmd {
        ContactsCommand::List(args) => {
            let contacts = client
                .contacts_list(&account, args.limit, args.search.as_deref())
                .map_err(|e| anyhow!("{e}"))?;
            emit_output(&ctx.common, &contacts)?;
        }
        ContactsCommand::Get(args) => {
            let contact = client
                .contacts_get(&account, &args.id)
                .map_err(|e| anyhow!("{e}"))?;
            emit_output(&ctx.common, &contact)?;
        }
        ContactsCommand::Create(args) => {
            let payload = read_json_payload(args.file.as_ref())?;
            let result = client
                .contacts_create(&account, payload)
                .map_err(|e| anyhow!("{e}"))?;
            emit_output(&ctx.common, &result)?;
        }
        ContactsCommand::Delete(args) => {
            let result = client
                .contacts_delete(&account, &args.id)
                .map_err(|e| anyhow!("{e}"))?;
            emit_output(&ctx.common, &result)?;
        }
    }
    Ok(())
}

fn handle_agenda(ctx: &RuntimeContext, args: AgendaArgs) -> Result<()> {
    let account = args.account.unwrap_or_else(|| effective_account(ctx));
    let client = ctx.service_client()?;

    // Today range in configured timezone
    let tz = ctx
        .config
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::UTC);
    let today = Local::now().with_timezone(&tz).date_naive();
    let start = today.and_hms_opt(0, 0, 0).unwrap();
    let end = today.and_hms_opt(23, 59, 59).unwrap();

    let events_val = client
        .calendar_list(
            &account,
            1,
            Some(&start.format("%Y-%m-%dT%H:%M:%S").to_string()),
            Some(&end.format("%Y-%m-%dT%H:%M:%S").to_string()),
        )
        .map_err(|e| anyhow!("{e}"))?;

    if ctx.common.json || ctx.common.yaml || !io::stdout().is_terminal() {
        emit_output(&ctx.common, &events_val)?;
        return Ok(());
    }

    let events: Vec<AgendaItem> =
        serde_json::from_value(events_val.clone()).context("parsing agenda items")?;
    render_agenda(&events, tz)?;
    Ok(())
}

fn handle_free(ctx: &RuntimeContext, cmd: FreeCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ctx.service_client()?;
    let slots = client
        .free_slots(&account, cmd.weeks, cmd.duration, cmd.limit)
        .map_err(|e| anyhow!("{e}"))?;
    emit_output(&ctx.common, &slots)?;
    Ok(())
}

fn handle_config(ctx: &RuntimeContext, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Show => emit_output(&ctx.common, &ctx.config),
        ConfigCommand::Path => {
            println!("{}", ctx.paths.global_config.display());
            Ok(())
        }
        ConfigCommand::Reset => {
            AppConfig::write_default(&ctx.paths.global_config).map_err(|e| anyhow!("{e}"))
        }
    }
}

fn handle_init(ctx: &RuntimeContext, cmd: InitCommand) -> Result<()> {
    if ctx.paths.global_config.exists() && !(cmd.force || ctx.common.assume_yes) {
        return Err(anyhow!(
            "config already exists at {} (use --force to overwrite)",
            ctx.paths.global_config.display()
        ));
    }
    if ctx.common.dry_run {
        println!(
            "dry-run: would write default config to {}",
            ctx.paths.global_config.display()
        );
        return Ok(());
    }
    AppConfig::write_default(&ctx.paths.global_config).map_err(|e| anyhow!("{e}"))
}

fn handle_completions(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, APP_NAME, &mut io::stdout());
    Ok(())
}

fn handle_service(ctx: &RuntimeContext, command: ServiceCommand) -> Result<()> {
    match command {
        ServiceCommand::Start => start_service(ctx),
        ServiceCommand::Stop => stop_service(ctx),
        ServiceCommand::Restart => restart_service(ctx),
        ServiceCommand::Status => status_service(ctx),
    }
}

fn effective_account(ctx: &RuntimeContext) -> String {
    ctx.common
        .account
        .clone()
        .unwrap_or_else(|| ctx.config.account.clone())
}

fn read_json_payload(path: Option<&PathBuf>) -> Result<Value> {
    let mut buffer = String::new();
    match path {
        Some(p) => {
            buffer = fs::read_to_string(p)
                .with_context(|| format!("reading payload from {}", p.display()))?;
        }
        None => {
            io::stdin()
                .read_to_string(&mut buffer)
                .context("reading JSON from stdin")?;
        }
    }
    let json: Value = serde_json::from_str(&buffer).context("parsing JSON payload")?;
    Ok(json)
}

fn emit_output<T: ?Sized + Serialize + std::fmt::Debug>(
    opts: &CommonOpts,
    value: &T,
) -> Result<()> {
    if opts.json {
        let json = serde_json::to_string_pretty(value)?;
        println!("{json}");
        return Ok(());
    }
    if opts.yaml {
        let yaml = serde_yaml::to_string(value)?;
        println!("{yaml}");
        return Ok(());
    }

    let v = serde_json::to_value(value)?;
    pretty_print_value(&v);
    Ok(())
}

fn pretty_print_value(v: &Value) {
    match v {
        Value::Array(items) => {
            for item in items {
                pretty_print_item(item);
            }
        }
        Value::Object(_) => pretty_print_item(v),
        _ => println!("{v}"),
    }
}

fn pretty_print_item(v: &Value) {
    let obj = match v {
        Value::Object(map) => map,
        _ => {
            println!("{v}");
            return;
        }
    };

    if obj.contains_key("subject") {
        let subject = obj
            .get("subject")
            .and_then(|v| v.as_str())
            .unwrap_or("No subject");
        println!("- {}", subject);
        if let Some(start) = obj.get("start").and_then(|v| v.as_str()) {
            println!("  Start: {}", start);
        }
        if let Some(end) = obj.get("end").and_then(|v| v.as_str()) {
            println!("  End: {}", end);
        }
        if let Some(loc) = obj.get("location").and_then(|v| v.as_str())
            && !loc.is_empty()
        {
            println!("  Location: {}", loc);
        }
        if let Some(from) = obj.get("from").and_then(|v| v.as_str()) {
            println!("  From: {}", from);
        }
        if let Some(dt) = obj.get("datetime_received").and_then(|v| v.as_str()) {
            println!("  Date: {}", dt);
        }
        println!();
        return;
    }

    if obj.contains_key("display_name") {
        let name = obj
            .get("display_name")
            .and_then(|v| v.as_str())
            .unwrap_or("No name");
        println!("- {}", name);
        if let Some(email) = obj.get("email").and_then(|v| v.as_str()) {
            println!("  Email: {}", email);
        }
        if let Some(phone) = obj.get("phone").and_then(|v| v.as_str()) {
            println!("  Phone: {}", phone);
        }
        if let Some(company) = obj.get("company").and_then(|v| v.as_str()) {
            println!("  Company: {}", company);
        }
        println!();
        return;
    }

    if obj.contains_key("duration_minutes") && obj.contains_key("day") {
        let start = obj
            .get("start")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let end = obj.get("end").and_then(|v| v.as_str()).unwrap_or_default();
        let day = obj.get("day").and_then(|v| v.as_str()).unwrap_or_default();
        let date = obj.get("date").and_then(|v| v.as_str()).unwrap_or_default();
        let dur = obj
            .get("duration_minutes")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        println!("- {day} {date}: {start} - {end} ({dur} min)");
        return;
    }

    println!("{:#?}", obj);
}

fn service_pid_path(ctx: &RuntimeContext) -> Result<PathBuf> {
    fs::create_dir_all(&ctx.paths.state_dir)
        .with_context(|| format!("creating state directory {}", ctx.paths.state_dir.display()))?;
    Ok(ctx.paths.state_dir.join("service.pid"))
}

fn read_pid(path: &std::path::Path) -> Result<Option<u32>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).context("reading pid file")?;
    let pid: u32 = text.trim().parse().context("parsing pid file")?;
    Ok(Some(pid))
}

fn start_service(ctx: &RuntimeContext) -> Result<()> {
    let pid_path = service_pid_path(ctx)?;
    if let Some(pid) = read_pid(&pid_path)?
        && pid_running(pid)
    {
        return Err(anyhow!("service already running with pid {}", pid));
    }

    let log_path = ctx.paths.state_dir.join("service.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening service log {}", log_path.display()))?;

    let child = ProcCommand::new("uv")
        .arg("run")
        .arg("h8-service")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .context("starting service with `uv run h8-service`")?;

    let pid = child.id();
    fs::write(&pid_path, pid.to_string()).context("writing pid file")?;
    println!(
        "service started (pid {}), logs: {}",
        pid,
        log_path.display()
    );
    Ok(())
}

fn stop_service(ctx: &RuntimeContext) -> Result<()> {
    let pid_path = service_pid_path(ctx)?;
    let Some(pid) = read_pid(&pid_path)? else {
        println!("service not running");
        return Ok(());
    };

    if !pid_running(pid) {
        println!("service not running (stale pid {})", pid);
        let _ = fs::remove_file(&pid_path);
        return Ok(());
    }

    terminate_pid(pid)?;
    let _ = fs::remove_file(&pid_path);
    println!("service stopped (pid {})", pid);
    Ok(())
}

fn restart_service(ctx: &RuntimeContext) -> Result<()> {
    let pid_path = service_pid_path(ctx)?;
    if let Some(pid) = read_pid(&pid_path)? {
        if pid_running(pid) {
            terminate_pid(pid)?;
            let _ = fs::remove_file(&pid_path);
            println!("service stopped (pid {})", pid);
        } else {
            let _ = fs::remove_file(&pid_path);
        }
    }
    start_service(ctx)
}

fn status_service(ctx: &RuntimeContext) -> Result<()> {
    let pid_path = service_pid_path(ctx)?;
    if let Some(pid) = read_pid(&pid_path)? {
        if pid_running(pid) {
            println!("service running (pid {})", pid);
        } else {
            println!("service pid file present but process not running ({})", pid);
        }
    } else {
        println!("service not running");
    }
    Ok(())
}

#[cfg(unix)]
fn pid_running(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_running(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn terminate_pid(pid: u32) -> Result<()> {
    let res = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if res == 0 {
        Ok(())
    } else {
        Err(anyhow!("failed to terminate pid {}", pid))
    }
}

#[cfg(not(unix))]
fn terminate_pid(pid: u32) -> Result<()> {
    ProcCommand::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .arg("/T")
        .arg("/F")
        .status()
        .map_err(|e| anyhow!(e))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(anyhow!("failed to terminate pid {}", pid))
            }
        })
}

fn render_agenda(events: &[AgendaItem], tz: chrono_tz::Tz) -> Result<()> {
    let today = Local::now().with_timezone(&tz).date_naive();
    let start_naive = today.and_hms_opt(0, 0, 0).unwrap();
    let end_naive = today.and_hms_opt(23, 59, 59).unwrap();
    let day_start = tz
        .from_local_datetime(&start_naive)
        .single()
        .unwrap_or_else(|| tz.from_utc_datetime(&start_naive));
    let day_end = tz
        .from_local_datetime(&end_naive)
        .single()
        .unwrap_or_else(|| tz.from_utc_datetime(&end_naive));

    struct AgendaSlot {
        subject: String,
        location: Option<String>,
        start_label: String,
        end_label: String,
        start_min: u32,
        end_min: u32,
        all_day: bool,
    }

    let mut slots = Vec::new();
    for ev in events {
        let subject = ev
            .subject
            .clone()
            .unwrap_or_else(|| "(no subject)".to_string());
        let is_all_day = ev.is_all_day.unwrap_or(false)
            || ev.start.as_deref().map(|s| s.len() == 10).unwrap_or(false);

        let start_dt = ev
            .start
            .as_deref()
            .and_then(|s| parse_datetime_local(s, tz));
        let end_dt = ev.end.as_deref().and_then(|s| parse_datetime_local(s, tz));

        let (start_dt, end_dt) = match (start_dt, end_dt, is_all_day) {
            (Some(s), Some(e), _) => (s, e),
            (_, _, true) => (day_start, day_end),
            _ => continue,
        };

        let start_min = ((start_dt - day_start).num_minutes()).clamp(0, 24 * 60) as u32;
        let mut end_min = ((end_dt - day_start).num_minutes()).clamp(0, 24 * 60) as u32;
        if end_min <= start_min {
            end_min = start_min + 1;
        }

        slots.push(AgendaSlot {
            subject,
            location: ev.location.clone(),
            start_label: start_dt.format("%H:%M").to_string(),
            end_label: end_dt.format("%H:%M").to_string(),
            start_min,
            end_min,
            all_day: is_all_day,
        });
    }

    println!("Agenda for {} ({})", today.format("%Y-%m-%d (%A)"), tz);
    println!("Times in {}", tz);

    if slots.is_empty() {
        println!("(no events today)");
        return Ok(());
    }

    slots.sort_by_key(|s| s.start_min);

    let width = 48usize;
    let minutes_per_tick = (24 * 60) / width as u32;

    for slot in slots {
        let start_tick = (slot.start_min / minutes_per_tick).min(width as u32 - 1);
        let end_tick = slot
            .end_min
            .div_ceil(minutes_per_tick)
            .clamp(start_tick + 1, width as u32);
        let mut bar = vec![' '; width];
        for idx in start_tick..end_tick {
            bar[idx as usize] = '=';
        }
        let bar: String = bar.into_iter().collect();
        let label = format!("{:>5}-{:>5}", slot.start_label, slot.end_label);
        println!("{label} | {bar} | {}", slot.subject);
        if let Some(loc) = slot.location.as_ref().filter(|s| !s.is_empty()) {
            println!("             {}", loc);
        }
        if slot.all_day {
            println!("             (all day)");
        }
    }

    Ok(())
}

fn parse_datetime_local(raw: &str, tz: chrono_tz::Tz) -> Option<DateTime<chrono_tz::Tz>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&tz));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S") {
        return tz
            .from_local_datetime(&dt)
            .single()
            .or_else(|| Some(tz.from_utc_datetime(&dt)));
    }
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0)?;
        return tz
            .from_local_datetime(&dt)
            .single()
            .or_else(|| Some(tz.from_utc_datetime(&dt)));
    }
    None
}
