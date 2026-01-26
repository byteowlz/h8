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
    DateTime, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc,
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
        Command::Addr(args) => handle_addr(&ctx, args),
        Command::Free(cmd) => handle_free(&ctx, cmd),
        Command::Ppl { command } => handle_ppl(&ctx, command),
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
    /// Search cached email addresses (from sent/received mail)
    #[command(alias = "address")]
    Addr(AddrArgs),
    Free(FreeCommand),
    /// Other people's calendar operations
    #[command(alias = "people")]
    Ppl {
        #[command(subcommand)]
        command: PplCommand,
    },
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
    /// Show events with natural language dates (e.g., 'next week', 'friday', 'kw30')
    Show(CalendarShowArgs),
    /// Add event with natural language (e.g., 'friday 2pm "Meeting" with alice')
    Add(CalendarAddArgs),
    /// Search events by subject, location, or body
    Search(CalendarSearchArgs),
    /// Send meeting invite to attendees
    Invite(CalendarInviteArgs),
    /// List pending meeting invites
    Invites(CalendarInvitesArgs),
    /// Respond to a meeting invite (accept/decline/tentative)
    Rsvp(CalendarRsvpArgs),
    Create(CalendarCreateArgs),
    /// Cancel a meeting and notify attendees
    Cancel(CalendarCancelArgs),
    Delete(CalendarDeleteArgs),
    /// Sync calendar events to local cache for fast access
    Sync(CalendarSyncArgs),
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
struct CalendarShowArgs {
    /// Date expression (e.g., today, mittwoch, 28.01, next week, kw30, feb 13, +2)
    #[arg(num_args = 0..)]
    when: Vec<String>,
}

#[derive(Debug, Args)]
struct CalendarAddArgs {
    /// Natural language event description (e.g., friday 2pm "Team Sync" with roman)
    #[arg(required = true, num_args = 1..)]
    input: Vec<String>,
    /// Default duration in minutes if not specified
    #[arg(short = 'd', long, default_value_t = 60)]
    duration: u32,
    /// Event location
    #[arg(short = 'l', long)]
    location: Option<String>,
}

#[derive(Debug, Args)]
struct CalendarCreateArgs {
    #[arg(long)]
    file: Option<PathBuf>,
    /// Input is in extraction/event.json schema format (from xtr)
    #[arg(short = 'e', long)]
    extracted: bool,
}

#[derive(Debug, Args)]
struct CalendarSearchArgs {
    /// Search query (matches subject, location, body)
    #[arg(required = true)]
    query: String,
    /// Number of days to search (default: 90)
    #[arg(short = 'd', long, default_value_t = 90)]
    days: i64,
    /// Start date (ISO format, e.g., 2026-01-01)
    #[arg(long = "from")]
    from_date: Option<String>,
    /// End date (ISO format)
    #[arg(long = "to")]
    to_date: Option<String>,
    /// Maximum results to return
    #[arg(short = 'n', long, default_value_t = 50)]
    limit: i64,
}

#[derive(Debug, Args)]
struct CalendarInviteArgs {
    /// Meeting subject
    #[arg(long, short = 's')]
    subject: String,
    /// Start time (ISO format, e.g., 2026-01-22T14:00:00)
    #[arg(long)]
    start: String,
    /// End time (ISO format)
    #[arg(long)]
    end: String,
    /// Required attendee email(s) - can be specified multiple times
    #[arg(long, short = 't')]
    to: Vec<String>,
    /// Optional attendee email(s) - can be specified multiple times
    #[arg(long, short = 'o')]
    optional: Vec<String>,
    /// Meeting location
    #[arg(long, short = 'l')]
    location: Option<String>,
    /// Meeting body/description
    #[arg(long, short = 'b')]
    body: Option<String>,
}

#[derive(Debug, Args)]
struct CalendarInvitesArgs {
    /// Maximum results to return
    #[arg(short = 'n', long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Args)]
struct CalendarRsvpArgs {
    /// Meeting invite ID
    id: String,
    /// Response: accept, decline, or tentative
    #[arg(value_enum)]
    response: RsvpResponse,
    /// Optional message to include with response
    #[arg(long, short = 'm')]
    message: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RsvpResponse {
    Accept,
    Decline,
    Tentative,
    Maybe,
}

impl RsvpResponse {
    fn as_str(&self) -> &'static str {
        match self {
            RsvpResponse::Accept => "accept",
            RsvpResponse::Decline => "decline",
            RsvpResponse::Tentative | RsvpResponse::Maybe => "tentative",
        }
    }
}

#[derive(Debug, Args)]
struct CalendarCancelArgs {
    /// Event ID to cancel (or use --query to cancel multiple)
    id: Option<String>,
    /// Search query to select events to cancel (e.g., "today", "standup")
    #[arg(short = 'q', long)]
    query: Option<String>,
    /// Optional cancellation message to send to attendees
    #[arg(short = 'm', long)]
    message: Option<String>,
    /// Dry run - show what would be cancelled
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct CalendarDeleteArgs {
    #[arg(long)]
    id: String,
    #[arg(long = "changekey")]
    change_key: Option<String>,
}

#[derive(Debug, Args)]
struct CalendarSyncArgs {
    /// Number of weeks to sync (default: 4)
    #[arg(short = 'w', long, default_value_t = 4)]
    weeks: i64,
    /// Also sync past events (weeks back, default: 1)
    #[arg(short = 'p', long, default_value_t = 1)]
    past_weeks: i64,
    /// Force full sync (ignore last sync time)
    #[arg(long)]
    full: bool,
}

#[derive(Debug, Subcommand)]
enum MailCommand {
    /// List messages in a folder
    #[command(alias = "ls")]
    List(MailListArgs),
    /// Search messages by subject, sender, or body
    Search(MailSearchArgs),
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
    /// Empty a folder (permanently delete all items)
    #[command(alias = "empty")]
    EmptyFolder(MailEmptyFolderArgs),
    /// Mark message(s) as spam/junk
    Spam(MailSpamArgs),
}

#[derive(Debug, Args)]
struct MailListArgs {
    /// Date filter (e.g., today, monday, mittwoch, 28.01, jan 15, +2)
    #[arg(num_args = 0..)]
    when: Vec<String>,
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    #[arg(short = 'l', long, default_value_t = 20)]
    limit: usize,
    #[arg(short = 'u', long)]
    unread: bool,
}

#[derive(Debug, Args)]
struct MailSearchArgs {
    /// Search query (subject, sender, body)
    #[arg(required = true)]
    query: String,
    /// Folder to search in
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    /// Maximum results to return
    #[arg(short = 'n', long, default_value_t = 50)]
    limit: i64,
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
    /// Date expression (e.g., today, tomorrow, mittwoch, feb 13, +2, 28.01)
    #[arg(num_args = 0..)]
    when: Vec<String>,
    #[arg(short = 'a', long = "account")]
    account: Option<String>,
    /// View mode: list, gantt, or compact (default from config)
    #[arg(short = 'V', long = "view", value_enum)]
    view: Option<AgendaView>,
}

/// Event status for visual indicators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum EventStatus {
    #[default]
    Normal,
    Cancelled,
    Blocker,
}

impl EventStatus {
    /// Detect status from subject prefixes/keywords.
    fn from_subject(subject: &str) -> Self {
        let lower = subject.to_lowercase();
        if lower.starts_with("cancelled:")
            || lower.starts_with("abgesagt:")
            || lower.contains("cancelled")
            || lower.contains("abgesagt")
        {
            EventStatus::Cancelled
        } else if lower.starts_with("blocker:")
            || lower.contains("blocker")
            || lower.starts_with("blocked:")
        {
            EventStatus::Blocker
        } else {
            EventStatus::Normal
        }
    }

    /// Nerd Font icon prefix for list view.
    fn icon(&self) -> &'static str {
        match self {
            EventStatus::Normal => "",
            EventStatus::Cancelled => "\u{f00d} ", // nf-fa-times (X)
            EventStatus::Blocker => "\u{f05e} ",   // nf-fa-ban (block icon)
        }
    }

    /// Unicode block character for gantt bars.
    fn bar_char(&self) -> char {
        match self {
            EventStatus::Normal => '\u{2588}',    // Full block
            EventStatus::Cancelled => '\u{2592}', // Medium shade (strikethrough effect)
            EventStatus::Blocker => '\u{2593}',   // Dark shade
        }
    }
}

/// Agenda view mode.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum AgendaView {
    /// Detailed list view with times and locations
    #[default]
    List,
    /// Gantt-style timeline chart
    Gantt,
    /// Compact view grouped by date
    Compact,
}

impl From<h8_core::CalendarView> for AgendaView {
    fn from(v: h8_core::CalendarView) -> Self {
        match v {
            h8_core::CalendarView::List => AgendaView::List,
            h8_core::CalendarView::Gantt => AgendaView::Gantt,
            h8_core::CalendarView::Compact => AgendaView::Compact,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AgendaItem {
    subject: Option<String>,
    start: Option<String>,
    end: Option<String>,
    location: Option<String>,
    is_all_day: Option<bool>,
}

/// Free/busy item from ppl agenda (other people's calendar)
#[derive(Debug, Deserialize)]
struct PplAgendaItem {
    start: Option<String>,
    end: Option<String>,
    status: Option<String>,
    subject: Option<String>,
    location: Option<String>,
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
    /// Draft ID to send (if not provided, uses --to/--subject/--body or reads from file/stdin)
    id: Option<String>,
    /// Read email from file instead of stdin
    #[arg(long)]
    file: Option<PathBuf>,
    /// Send all drafts
    #[arg(long)]
    all: bool,
    /// Schedule delivery (e.g., "tomorrow 9am", "friday 14:00", "2026-01-20 10:30")
    #[arg(long, short = 's')]
    schedule: Option<String>,

    // Direct composition flags (for programmatic/agent use)
    /// Recipient email address(es) - can be specified multiple times
    #[arg(long, short = 't')]
    to: Vec<String>,
    /// CC recipient(s) - can be specified multiple times
    #[arg(long, short = 'c')]
    cc: Vec<String>,
    /// BCC recipient(s) - can be specified multiple times
    #[arg(long)]
    bcc: Vec<String>,
    /// Email subject
    #[arg(long)]
    subject: Option<String>,
    /// Email body (use "-" to read from stdin)
    #[arg(long, short = 'b')]
    body: Option<String>,
    /// Treat body as HTML
    #[arg(long)]
    html: bool,
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
    /// Message ID(s) to move (space or comma separated). Optional if --query is used.
    #[arg(num_args = 0..)]
    ids: Vec<String>,
    /// Target folder (use --to or natural "to" keyword)
    #[arg(short = 't', long = "to")]
    target: Option<String>,
    /// Source folder
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    /// Search query to select messages (e.g., "from:newsletter", "subject:weekly")
    #[arg(short = 'q', long)]
    query: Option<String>,
    /// Maximum messages to move when using --query
    #[arg(short = 'n', long, default_value_t = 50)]
    limit: i64,
    /// Create target folder if it doesn't exist (default: true)
    #[arg(short = 'c', long, default_value_t = true)]
    create: bool,
    /// Sync move to server (default: true)
    #[arg(long, default_value_t = true)]
    sync: bool,
    /// Dry run - show what would be moved without actually moving
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct MailDeleteArgs {
    /// Message ID(s) to delete (space or comma separated)
    #[arg(required = true, num_args = 1..)]
    ids: Vec<String>,
    /// Folder containing the message(s)
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    /// Permanently delete (skip trash)
    #[arg(long)]
    force: bool,
    /// Sync deletion to server (default: true)
    #[arg(long, default_value_t = true)]
    sync: bool,
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

#[derive(Debug, Args)]
struct MailEmptyFolderArgs {
    /// Folder to empty (default: trash)
    #[arg(default_value = "trash")]
    folder: String,
    /// Skip confirmation prompt
    #[arg(short = 'y', long)]
    yes: bool,
}

#[derive(Debug, Args)]
struct MailSpamArgs {
    /// Message ID(s) to mark as spam (space or comma separated)
    #[arg(required = true, num_args = 1..)]
    ids: Vec<String>,
    /// Mark as NOT spam (move to inbox instead)
    #[arg(long)]
    not_spam: bool,
    /// Only mark, don't move to junk/inbox folder
    #[arg(long)]
    no_move: bool,
}

#[derive(Debug, Subcommand)]
enum ContactsCommand {
    #[command(alias = "ls")]
    List(ContactsListArgs),
    Get(ContactsGetArgs),
    Create(ContactsCreateArgs),
    /// Update an existing contact
    Update(ContactsUpdateArgs),
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
struct ContactsUpdateArgs {
    /// Contact ID to update
    #[arg(long)]
    id: String,
    /// Display name
    #[arg(long)]
    name: Option<String>,
    /// Given/first name
    #[arg(long)]
    given_name: Option<String>,
    /// Surname/last name
    #[arg(long)]
    surname: Option<String>,
    /// Email address
    #[arg(long)]
    email: Option<String>,
    /// Phone number
    #[arg(long)]
    phone: Option<String>,
    /// Company name
    #[arg(long)]
    company: Option<String>,
    /// Job title
    #[arg(long)]
    job_title: Option<String>,
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
    /// View mode: list, gantt, or compact (default from config)
    #[arg(short = 'V', long = "view", value_enum)]
    view: Option<AgendaView>,
}

#[derive(Debug, Args)]
struct AddrArgs {
    /// Search query (matches email or name)
    query: Option<String>,
    /// Maximum results
    #[arg(short = 'l', long, default_value_t = 20)]
    limit: usize,
    /// Show most frequently used (ignore query)
    #[arg(long)]
    frequent: bool,
}

#[derive(Debug, Subcommand)]
enum PplCommand {
    /// View another person's calendar events
    Agenda(PplAgendaArgs),
    /// Find free slots in another person's calendar
    Free(PplFreeArgs),
    /// Find common free slots between multiple people
    Common(PplCommonArgs),
}

#[derive(Debug, Args)]
struct PplAgendaArgs {
    /// Person alias or email address
    person: String,
    #[arg(short = 'd', long, default_value_t = 7)]
    days: i64,
    #[arg(long = "from")]
    from_date: Option<String>,
    #[arg(long = "to")]
    to_date: Option<String>,
    /// View mode: list, gantt, or compact (default from config)
    #[arg(short = 'V', long = "view", value_enum)]
    view: Option<AgendaView>,
}

#[derive(Debug, Args)]
struct PplFreeArgs {
    /// Person alias or email address
    person: String,
    #[arg(short = 'w', long, default_value_t = 1)]
    weeks: u8,
    #[arg(short = 'd', long, default_value_t = 30)]
    duration: u32,
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    /// View mode: list, gantt, or compact (default from config)
    #[arg(short = 'V', long = "view", value_enum)]
    view: Option<AgendaView>,
}

#[derive(Debug, Args)]
struct PplCommonArgs {
    /// Person aliases or email addresses (2 or more)
    #[arg(required = true, num_args = 2..)]
    people: Vec<String>,
    #[arg(short = 'w', long, default_value_t = 1)]
    weeks: u8,
    #[arg(short = 'd', long, default_value_t = 30)]
    duration: u32,
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    /// View mode: list, gantt, or compact (default from config)
    #[arg(short = 'V', long = "view", value_enum)]
    view: Option<AgendaView>,
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

            // Sync events to local DB and assign word IDs
            let events_with_ids = sync_calendar_events(ctx, &account, &events)?;
            emit_output(&ctx.common, &events_with_ids)?;
        }
        CalendarCommand::Create(args) => {
            let payload = read_json_payload(args.file.as_ref())?;
            let event = client
                .calendar_create(&account, payload)
                .map_err(|e| anyhow!("{e}"))?;

            // Sync newly created event
            let events_with_ids = sync_calendar_events(ctx, &account, &serde_json::json!([event]))?;
            if let Some(e) = events_with_ids.as_array().and_then(|a| a.first()) {
                emit_output(&ctx.common, e)?;
            } else {
                emit_output(&ctx.common, &event)?;
            }
        }
        CalendarCommand::Cancel(args) => {
            // Get events to cancel - either by ID or by query
            let events_to_cancel: Vec<(String, String, String)> = if let Some(ref query) = args.query {
                // Search for events matching query
                let (from_date, to_date, _) = parse_date_range_expr(query);
                let results = client
                    .calendar_list(&account, 0, Some(&from_date), Some(&to_date))
                    .map_err(|e| anyhow!("{e}"))?;

                results
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| {
                                let id = e.get("id").and_then(|v| v.as_str())?;
                                let subject = e.get("subject").and_then(|v| v.as_str()).unwrap_or("(no subject)");
                                let start = e.get("start").and_then(|v| v.as_str()).unwrap_or("");
                                Some((id.to_string(), subject.to_string(), start.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else if let Some(ref id) = args.id {
                let remote_id = resolve_calendar_id(ctx, &account, id)?;
                vec![(remote_id, id.clone(), String::new())]
            } else {
                return Err(anyhow!("provide event ID or --query"));
            };

            if events_to_cancel.is_empty() {
                println!("No events found to cancel");
                return Ok(());
            }

            // Show what will be cancelled
            println!("Events to cancel ({}):", events_to_cancel.len());
            for (_, subject, start) in &events_to_cancel {
                if start.is_empty() {
                    println!("  - {}", subject);
                } else {
                    println!("  - {} ({})", subject, start);
                }
            }

            if args.dry_run {
                println!("\nDry run - no events cancelled");
                return Ok(());
            }

            println!();

            // Cancel each event
            let mut cancelled = 0;
            let mut errors: Vec<String> = Vec::new();
            let db_path = ctx.paths.sync_db_path(&account);
            let db = Database::open(&db_path).ok();

            for (remote_id, subject, _) in &events_to_cancel {
                match client.calendar_cancel(&account, remote_id, args.message.as_deref()) {
                    Ok(result) => {
                        if result.get("success").and_then(|v| v.as_bool()) == Some(true) {
                            println!("Cancelled: {}", subject);
                            cancelled += 1;

                            // Free the word ID if we have a DB
                            if let Some(ref database) = db {
                                let _ = database.delete_calendar_event(remote_id);
                                let id_gen = IdGenerator::new(database);
                                let _ = id_gen.free(remote_id);
                            }
                        } else {
                            let err = result.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
                            errors.push(format!("{}: {}", subject, err));
                        }
                    }
                    Err(e) => errors.push(format!("{}: {}", subject, e)),
                }
            }

            if !errors.is_empty() {
                for err in &errors {
                    eprintln!("Error: {}", err);
                }
            }

            println!("\nCancelled {} of {} event(s)", cancelled, events_to_cancel.len());
        }
        CalendarCommand::Delete(args) => {
            // Resolve word ID to remote ID if needed
            let remote_id = resolve_calendar_id(ctx, &account, &args.id)?;
            let result = client
                .calendar_delete(&account, &remote_id, args.change_key.as_deref())
                .map_err(|e| anyhow!("{e}"))?;

            // Free the word ID
            let db_path = ctx.paths.sync_db_path(&account);
            if let Ok(db) = Database::open(&db_path) {
                let _ = db.delete_calendar_event(&args.id);
                let id_gen = IdGenerator::new(&db);
                let _ = id_gen.free(&args.id);
            }

            emit_output(&ctx.common, &result)?;
        }
        CalendarCommand::Search(args) => {
            let events = client
                .calendar_search(
                    &account,
                    &args.query,
                    args.days,
                    args.from_date.as_deref(),
                    args.to_date.as_deref(),
                    args.limit,
                )
                .map_err(|e| anyhow!("{e}"))?;

            // Sync events to local DB and assign word IDs
            let events_with_ids = sync_calendar_events(ctx, &account, &events)?;

            if !ctx.common.json && !ctx.common.yaml {
                let count = events_with_ids.as_array().map(|a| a.len()).unwrap_or(0);
                println!("Found {} event(s) matching \"{}\":\n", count, args.query);
            }

            emit_output(&ctx.common, &events_with_ids)?;
        }
        CalendarCommand::Show(args) => {
            let when_text = if args.when.is_empty() {
                "today".to_string()
            } else {
                args.when.join(" ")
            };

            let (from_date, to_date, description) = parse_date_range_expr(&when_text);

            // Try local cache first for single-day queries
            let db_path = ctx.paths.sync_db_path(&account);
            let events_with_ids = if db_path.exists() && from_date == to_date {
                let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
                let cached_events = db
                    .list_calendar_events_range(&from_date, &to_date)
                    .map_err(|e| anyhow!("{e}"))?;

                if !cached_events.is_empty() {
                    // Convert cached events to JSON format
                    let events_json: Vec<serde_json::Value> = cached_events
                        .into_iter()
                        .map(|e| {
                            serde_json::json!({
                                "id": e.local_id,
                                "remote_id": e.remote_id,
                                "subject": e.subject,
                                "location": e.location,
                                "start": e.start,
                                "end": e.end,
                                "is_all_day": e.is_all_day,
                            })
                        })
                        .collect();
                    serde_json::json!(events_json)
                } else {
                    // No cached events, fetch from server
                    let events = client
                        .calendar_list(&account, 0, Some(&from_date), Some(&to_date))
                        .map_err(|e| anyhow!("{e}"))?;
                    sync_calendar_events(ctx, &account, &events)?
                }
            } else {
                // Range query or no cache - fetch from server
                let events = client
                    .calendar_list(&account, 0, Some(&from_date), Some(&to_date))
                    .map_err(|e| anyhow!("{e}"))?;
                sync_calendar_events(ctx, &account, &events)?
            };

            if !ctx.common.json && !ctx.common.yaml {
                println!("Events for {}:", description);
                println!("({} to {})\n", from_date, to_date);
            }

            emit_output(&ctx.common, &events_with_ids)?;
        }
        CalendarCommand::Add(args) => {
            let input_text = args.input.join(" ");

            // Parse the natural language input via the service
            let payload = client
                .calendar_parse_natural(&account, &input_text, args.duration, args.location.as_deref())
                .map_err(|e| anyhow!("{e}"))?;

            // Create the event
            let event = client
                .calendar_create(&account, payload.clone())
                .map_err(|e| anyhow!("{e}"))?;

            // Sync newly created event
            let events_with_ids = sync_calendar_events(ctx, &account, &serde_json::json!([event]))?;

            if !ctx.common.json && !ctx.common.yaml {
                let subject = payload.get("subject").and_then(|v| v.as_str()).unwrap_or("Event");
                let start = payload.get("start").and_then(|v| v.as_str()).unwrap_or("");
                let end = payload.get("end").and_then(|v| v.as_str()).unwrap_or("");
                println!("Created: {}", subject);
                println!("  When: {} - {}", start, end);
                if let Some(loc) = args.location.as_ref() {
                    println!("  Where: {}", loc);
                }
            } else if let Some(e) = events_with_ids.as_array().and_then(|a| a.first()) {
                emit_output(&ctx.common, e)?;
            }
        }
        CalendarCommand::Invite(args) => {
            let payload = serde_json::json!({
                "subject": args.subject,
                "start": args.start,
                "end": args.end,
                "required_attendees": args.to,
                "optional_attendees": args.optional,
                "location": args.location,
                "body": args.body,
            });

            let result = client
                .calendar_invite(&account, payload)
                .map_err(|e| anyhow!("{e}"))?;

            if !ctx.common.json && !ctx.common.yaml {
                println!("Meeting invite sent: {}", args.subject);
                println!("  When: {} - {}", args.start, args.end);
                if !args.to.is_empty() {
                    println!("  To: {}", args.to.join(", "));
                }
                if !args.optional.is_empty() {
                    println!("  Optional: {}", args.optional.join(", "));
                }
            }
            emit_output(&ctx.common, &result)?;
        }
        CalendarCommand::Invites(args) => {
            let invites = client
                .calendar_invites(&account, args.limit)
                .map_err(|e| anyhow!("{e}"))?;

            if !ctx.common.json && !ctx.common.yaml {
                let count = invites.as_array().map(|a| a.len()).unwrap_or(0);
                if count == 0 {
                    println!("No pending meeting invites.");
                } else {
                    println!("Pending meeting invites ({}):\n", count);
                }
            }

            emit_output(&ctx.common, &invites)?;
        }
        CalendarCommand::Rsvp(args) => {
            let result = client
                .calendar_rsvp(&account, &args.id, args.response.as_str(), args.message.as_deref())
                .map_err(|e| anyhow!("{e}"))?;

            if !ctx.common.json && !ctx.common.yaml {
                let subject = result.get("subject").and_then(|v| v.as_str()).unwrap_or("Meeting");
                println!("Responded '{}' to: {}", args.response.as_str(), subject);
            }
            emit_output(&ctx.common, &result)?;
        }
        CalendarCommand::Sync(args) => {
            handle_calendar_sync(ctx, &client, &account, args)?;
        }
    }
    Ok(())
}

/// Sync calendar events to local DB and assign word IDs.
fn sync_calendar_events(ctx: &RuntimeContext, account: &str, events: &Value) -> Result<Value> {
    use h8_core::types::CalendarEventSync;

    let db_path = ctx.paths.sync_db_path(account);
    let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
    let id_gen = IdGenerator::new(&db);

    // Ensure ID pool is seeded
    let stats = id_gen.stats().map_err(|e| anyhow!("{e}"))?;
    if stats.total() == 0 {
        let words = h8_core::id::WordLists::embedded();
        id_gen.init_pool(&words).map_err(|e| anyhow!("{e}"))?;
    }

    let events_array = match events.as_array() {
        Some(arr) => arr,
        None => return Ok(events.clone()),
    };

    let mut result = Vec::new();
    let now = chrono::Utc::now().to_rfc3339();

    for event in events_array {
        let remote_id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if remote_id.is_empty() {
            result.push(event.clone());
            continue;
        }

        // Check if we already have this event
        let local_id = if let Ok(Some(existing)) = db.get_calendar_event_by_remote_id(remote_id) {
            existing.local_id
        } else if let Ok(Some(existing_id)) = id_gen.reverse_lookup(remote_id) {
            existing_id
        } else {
            // Allocate new word ID
            id_gen.allocate(remote_id).map_err(|e| anyhow!("{e}"))?
        };

        // Determine if all-day event
        let start = event.get("start").and_then(|v| v.as_str()).unwrap_or("");
        let is_all_day = !start.contains('T');

        // Save to database
        let event_sync = CalendarEventSync {
            local_id: local_id.clone(),
            remote_id: remote_id.to_string(),
            change_key: event
                .get("changekey")
                .and_then(|v| v.as_str())
                .map(String::from),
            subject: event
                .get("subject")
                .and_then(|v| v.as_str())
                .map(String::from),
            location: event
                .get("location")
                .and_then(|v| v.as_str())
                .map(String::from),
            start: event
                .get("start")
                .and_then(|v| v.as_str())
                .map(String::from),
            end: event.get("end").and_then(|v| v.as_str()).map(String::from),
            is_all_day,
            synced_at: Some(now.clone()),
        };
        db.upsert_calendar_event(&event_sync)
            .map_err(|e| anyhow!("{e}"))?;

        // Create output event with word ID
        let mut out_event = event.clone();
        if let Some(obj) = out_event.as_object_mut() {
            obj.insert("id".to_string(), serde_json::json!(local_id));
        }
        result.push(out_event);
    }

    Ok(serde_json::json!(result))
}

/// Resolve a calendar ID (word ID or remote ID) to a remote ID.
fn resolve_calendar_id(ctx: &RuntimeContext, account: &str, id: &str) -> Result<String> {
    // If it looks like a word ID (contains hyphen, short), try to resolve it
    if id.contains('-') && id.len() < 30 {
        let db_path = ctx.paths.sync_db_path(account);
        if let Ok(db) = Database::open(&db_path) {
            if let Ok(Some(event)) = db.get_calendar_event(id) {
                return Ok(event.remote_id);
            }
            let id_gen = IdGenerator::new(&db);
            if let Ok(Some(remote)) = id_gen.resolve(id) {
                return Ok(remote);
            }
        }
    }
    // Assume it's already a remote ID
    Ok(id.to_string())
}

/// Sync calendar events from server to local cache.
fn handle_calendar_sync(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: CalendarSyncArgs,
) -> Result<()> {
    use h8_core::types::CalendarEventSync;

    let db_path = ctx.paths.sync_db_path(account);
    let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
    let id_gen = IdGenerator::new(&db);

    // Ensure ID pool is seeded
    let stats = id_gen.stats().map_err(|e| anyhow!("{e}"))?;
    if stats.total() == 0 {
        let words = h8_core::id::WordLists::embedded();
        id_gen.init_pool(&words).map_err(|e| anyhow!("{e}"))?;
    }

    // Calculate date range
    let now = Local::now();
    let start_date = (now - ChronoDuration::weeks(args.past_weeks))
        .format("%Y-%m-%dT00:00:00")
        .to_string();
    let end_date = (now + ChronoDuration::weeks(args.weeks))
        .format("%Y-%m-%dT23:59:59")
        .to_string();

    if !ctx.common.quiet {
        println!(
            "Syncing calendar events from {} to {}...",
            &start_date[..10],
            &end_date[..10]
        );
    }

    // Fetch events from server
    let events = client
        .calendar_list(account, 0, Some(&start_date), Some(&end_date))
        .map_err(|e| anyhow!("{e}"))?;

    let events_array = events.as_array().ok_or_else(|| anyhow!("expected array"))?;

    if !ctx.common.quiet {
        println!("Found {} events", events_array.len());
    }

    let sync_time = chrono::Utc::now().to_rfc3339();
    let mut synced = 0;

    for event in events_array {
        let remote_id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if remote_id.is_empty() {
            continue;
        }

        // Check if we already have this event
        let local_id = if let Ok(Some(existing)) = db.get_calendar_event_by_remote_id(remote_id) {
            existing.local_id
        } else if let Ok(Some(existing_id)) = id_gen.reverse_lookup(remote_id) {
            existing_id
        } else {
            // Allocate new word ID
            id_gen.allocate(remote_id).map_err(|e| anyhow!("{e}"))?
        };

        // Determine if all-day event
        let start = event.get("start").and_then(|v| v.as_str()).unwrap_or("");
        let is_all_day = !start.contains('T');

        // Save to database
        let event_sync = CalendarEventSync {
            local_id: local_id.clone(),
            remote_id: remote_id.to_string(),
            change_key: event
                .get("changekey")
                .and_then(|v| v.as_str())
                .map(String::from),
            subject: event
                .get("subject")
                .and_then(|v| v.as_str())
                .map(String::from),
            location: event
                .get("location")
                .and_then(|v| v.as_str())
                .map(String::from),
            start: event
                .get("start")
                .and_then(|v| v.as_str())
                .map(String::from),
            end: event.get("end").and_then(|v| v.as_str()).map(String::from),
            is_all_day,
            synced_at: Some(sync_time.clone()),
        };
        db.upsert_calendar_event(&event_sync)
            .map_err(|e| anyhow!("{e}"))?;
        synced += 1;
    }

    // Update sync state
    db.set_calendar_sync_state(&sync_time)
        .map_err(|e| anyhow!("{e}"))?;

    // Optionally clean up old events
    let cleanup_before = (now - ChronoDuration::weeks(args.past_weeks + 4))
        .format("%Y-%m-%dT00:00:00")
        .to_string();
    let deleted = db
        .delete_old_calendar_events(&cleanup_before)
        .map_err(|e| anyhow!("{e}"))?;

    if !ctx.common.quiet {
        println!("Synced {} events", synced);
        if deleted > 0 {
            println!("Cleaned up {} old events", deleted);
        }
        if let Ok(Some(last_sync)) = db.get_calendar_sync_state() {
            println!("Last sync: {}", &last_sync[..19]);
        }
    }

    Ok(())
}

// =============================================================================
// Unified Date Parsing
// =============================================================================

/// Parse a single date from natural language expression.
///
/// Returns (date, description) or None if not parseable.
///
/// Supported formats:
/// - Relative: "today", "heute", "yesterday", "gestern", "tomorrow", "morgen",
///   "overmorrow", "uebermorgen", "übermorgen"
/// - Offset: "+2", "-1", "+0" (days from today)
/// - Weekdays: "monday", "mon", "montag", "mittwoch", etc. (most recent occurrence)
/// - ISO: "2026-01-28"
/// - Slash: "2026/01/28"
/// - German dot: "28.01", "28.01.2026", "28.1", "28.1.26"
/// - Month+day: "jan 28", "28 jan", "january 28", "28. januar"
/// - Bare day: "28" (current month, or previous month if date passed)
fn parse_single_date(text: &str) -> Option<(NaiveDate, String)> {
    use chrono::{Datelike, Weekday};
    use regex::Regex;

    let now = Local::now();
    let today = now.date_naive();
    let text_lower = text.to_lowercase().trim().to_string();

    if text_lower.is_empty() {
        return None;
    }

    // 1. Offset format: "+2", "-1", "+0"
    let offset_re = Regex::new(r"^([+-])(\d+)$").unwrap();
    if let Some(caps) = offset_re.captures(&text_lower) {
        let sign = caps.get(1).unwrap().as_str();
        let num: i64 = caps.get(2).unwrap().as_str().parse().ok()?;
        let offset = if sign == "-" { -num } else { num };
        let target = today + ChronoDuration::days(offset);
        let desc = if offset == 0 {
            "today".to_string()
        } else if offset == 1 {
            "tomorrow".to_string()
        } else if offset == -1 {
            "yesterday".to_string()
        } else {
            format!("{:+} days", offset)
        };
        return Some((target, desc));
    }

    // 2. Relative days (including uebermorgen without umlaut)
    let relative_days: &[(&str, i64)] = &[
        ("today", 0),
        ("heute", 0),
        ("yesterday", -1),
        ("gestern", -1),
        ("tomorrow", 1),
        ("morgen", 1),
        ("overmorrow", 2),
        ("übermorgen", 2),
        ("uebermorgen", 2),
    ];
    for (keyword, offset) in relative_days {
        if text_lower == *keyword {
            let target = today + ChronoDuration::days(*offset);
            return Some((target, (*keyword).to_string()));
        }
    }

    // 3. Weekday names (returns most recent occurrence, including today)
    let weekdays: &[(&str, Weekday)] = &[
        ("monday", Weekday::Mon),
        ("mon", Weekday::Mon),
        ("montag", Weekday::Mon),
        ("tuesday", Weekday::Tue),
        ("tue", Weekday::Tue),
        ("dienstag", Weekday::Tue),
        ("wednesday", Weekday::Wed),
        ("wed", Weekday::Wed),
        ("mittwoch", Weekday::Wed),
        ("thursday", Weekday::Thu),
        ("thu", Weekday::Thu),
        ("donnerstag", Weekday::Thu),
        ("friday", Weekday::Fri),
        ("fri", Weekday::Fri),
        ("freitag", Weekday::Fri),
        ("saturday", Weekday::Sat),
        ("sat", Weekday::Sat),
        ("samstag", Weekday::Sat),
        ("sunday", Weekday::Sun),
        ("sun", Weekday::Sun),
        ("sonntag", Weekday::Sun),
    ];
    for (name, weekday) in weekdays {
        if text_lower == *name {
            let today_weekday = today.weekday();
            // Find this week's occurrence of the weekday
            // Week runs Mon-Sun, so we calculate offset from Monday
            let today_offset = today_weekday.num_days_from_monday() as i64;
            let target_offset = weekday.num_days_from_monday() as i64;
            let days_diff = target_offset - today_offset;
            let target = today + ChronoDuration::days(days_diff);
            return Some((target, (*name).to_string()));
        }
    }

    // 4. German dot format: "28.01", "28.01.2026", "28.1", "28.1.26"
    let dot_re = Regex::new(r"^(\d{1,2})\.(\d{1,2})(?:\.(\d{2,4}))?$").unwrap();
    if let Some(caps) = dot_re.captures(&text_lower) {
        let day: u32 = caps.get(1).unwrap().as_str().parse().ok()?;
        let month: u32 = caps.get(2).unwrap().as_str().parse().ok()?;
        let year: i32 = if let Some(y) = caps.get(3) {
            let y_str = y.as_str();
            let y_num: i32 = y_str.parse().ok()?;
            if y_num < 100 {
                2000 + y_num // "26" -> 2026
            } else {
                y_num
            }
        } else {
            // No year: use current year, or previous year if date is in the future
            let current_year = now.year();
            if month > now.month() || (month == now.month() && day > now.day()) {
                current_year - 1
            } else {
                current_year
            }
        };

        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            return Some((date, date.format("%d.%m.%Y").to_string()));
        }
    }

    // 5. Slash date format: "2025/02/01"
    let slash_re = Regex::new(r"^(\d{4})/(\d{1,2})/(\d{1,2})$").unwrap();
    if let Some(caps) = slash_re.captures(&text_lower) {
        let year: i32 = caps.get(1).unwrap().as_str().parse().ok()?;
        let month: u32 = caps.get(2).unwrap().as_str().parse().ok()?;
        let day: u32 = caps.get(3).unwrap().as_str().parse().ok()?;
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            return Some((date, date.format("%B %d, %Y").to_string()));
        }
    }

    // 6. ISO date format: "2025-02-01"
    if let Ok(date) = NaiveDate::parse_from_str(&text_lower, "%Y-%m-%d") {
        return Some((date, date.format("%B %d, %Y").to_string()));
    }

    // 7. Month names with optional day
    let months: &[(&str, u32)] = &[
        ("january", 1),
        ("jan", 1),
        ("januar", 1),
        ("february", 2),
        ("feb", 2),
        ("februar", 2),
        ("march", 3),
        ("mar", 3),
        ("märz", 3),
        ("maerz", 3),
        ("april", 4),
        ("apr", 4),
        ("may", 5),
        ("mai", 5),
        ("june", 6),
        ("jun", 6),
        ("juni", 6),
        ("july", 7),
        ("jul", 7),
        ("juli", 7),
        ("august", 8),
        ("aug", 8),
        ("september", 9),
        ("sep", 9),
        ("sept", 9),
        ("october", 10),
        ("oct", 10),
        ("okt", 10),
        ("oktober", 10),
        ("november", 11),
        ("nov", 11),
        ("december", 12),
        ("dec", 12),
        ("dez", 12),
        ("dezember", 12),
    ];

    for (month_name, month_num) in months {
        if text_lower.contains(month_name) {
            // Extract day and optional year: "28", "28 2024", "28.", etc.
            let day_year_re = Regex::new(r"(\d{1,2})\.?(?:\s+(\d{4}))?").unwrap();
            if let Some(caps) = day_year_re.captures(&text_lower) {
                let day: u32 = caps.get(1).unwrap().as_str().parse().ok()?;
                let year: i32 = if let Some(y) = caps.get(2) {
                    y.as_str().parse().ok()?
                } else {
                    // Use current year, or previous year if the date is in the future
                    let current_year = now.year();
                    if *month_num > now.month()
                        || (*month_num == now.month() && day > now.day())
                    {
                        current_year - 1
                    } else {
                        current_year
                    }
                };

                if let Some(date) = NaiveDate::from_ymd_opt(year, *month_num, day) {
                    return Some((date, date.format("%B %d, %Y").to_string()));
                }
            }
        }
    }

    // 8. Bare day number: "28" (current month, or previous month if passed)
    let bare_day_re = Regex::new(r"^(\d{1,2})$").unwrap();
    if let Some(caps) = bare_day_re.captures(&text_lower) {
        let day: u32 = caps.get(1).unwrap().as_str().parse().ok()?;
        if (1..=31).contains(&day) {
            let mut year = now.year();
            let mut month = now.month();

            // If day is in the future this month, look at previous month
            if day > now.day() {
                if month == 1 {
                    month = 12;
                    year -= 1;
                } else {
                    month -= 1;
                }
            }

            if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
                return Some((date, date.format("%B %d").to_string()));
            }
        }
    }

    None
}

/// Parse a natural language date range expression for calendar viewing.
///
/// Returns (from_date, to_date, description) as ISO date strings and human-readable description.
///
/// Supported expressions:
/// - All single-date formats from parse_single_date
/// - "next week", "nächste woche" - Monday to Sunday of next week
/// - "this week", "diese woche" - rest of current week
/// - "kw30", "week 30" - calendar week 30
/// - "december", "dezember" - entire month (when no day specified)
fn parse_date_range_expr(text: &str) -> (String, String, String) {
    use chrono::Datelike;
    use regex::Regex;

    let now = Local::now();
    let today = now.date_naive();
    let text_lower = text.to_lowercase();

    // 1. Check for week number: "kw30", "kw 30", "week 30", "woche 30"
    let week_re = Regex::new(r"(?i)\b(?:kw|week|woche)\s*(\d{1,2})\b").unwrap();
    if let Some(caps) = week_re.captures(&text_lower) {
        if let Ok(week_num) = caps.get(1).unwrap().as_str().parse::<u32>() {
            let year = now.year();
            // ISO week: find the Monday of week 1, then add weeks
            let jan4 = NaiveDate::from_ymd_opt(year, 1, 4).unwrap();
            let week1_monday =
                jan4 - ChronoDuration::days(jan4.weekday().num_days_from_monday() as i64);
            let start = week1_monday + ChronoDuration::weeks((week_num - 1) as i64);
            let end = start + ChronoDuration::days(6);
            return (
                start.format("%Y-%m-%d").to_string(),
                end.format("%Y-%m-%d").to_string(),
                format!("KW{} {}", week_num, year),
            );
        }
    }

    // 2. Check for "next week" / "nächste woche" / "naechste woche"
    let next_week_re =
        Regex::new(r"(?i)\b(next\s+week|nächste\s+woche|naechste\s+woche)\b").unwrap();
    if next_week_re.is_match(&text_lower) {
        let days_until_monday = (7 - now.weekday().num_days_from_monday()) % 7;
        let days_until_monday = if days_until_monday == 0 {
            7
        } else {
            days_until_monday
        };
        let next_monday = today + ChronoDuration::days(days_until_monday as i64);
        let next_sunday = next_monday + ChronoDuration::days(6);
        return (
            next_monday.format("%Y-%m-%d").to_string(),
            next_sunday.format("%Y-%m-%d").to_string(),
            "next week".to_string(),
        );
    }

    // 3. Check for "this week" / "diese woche"
    let this_week_re = Regex::new(r"(?i)\b(this\s+week|diese\s+woche)\b").unwrap();
    if this_week_re.is_match(&text_lower) {
        let days_until_sunday = 6 - now.weekday().num_days_from_monday();
        let sunday = today + ChronoDuration::days(days_until_sunday as i64);
        return (
            today.format("%Y-%m-%d").to_string(),
            sunday.format("%Y-%m-%d").to_string(),
            "this week".to_string(),
        );
    }

    // 4. Month names without day (entire month)
    let months: &[(&str, u32)] = &[
        ("january", 1),
        ("jan", 1),
        ("januar", 1),
        ("february", 2),
        ("feb", 2),
        ("februar", 2),
        ("march", 3),
        ("mar", 3),
        ("märz", 3),
        ("maerz", 3),
        ("april", 4),
        ("apr", 4),
        ("may", 5),
        ("mai", 5),
        ("june", 6),
        ("jun", 6),
        ("juni", 6),
        ("july", 7),
        ("jul", 7),
        ("juli", 7),
        ("august", 8),
        ("aug", 8),
        ("september", 9),
        ("sep", 9),
        ("sept", 9),
        ("october", 10),
        ("oct", 10),
        ("okt", 10),
        ("oktober", 10),
        ("november", 11),
        ("nov", 11),
        ("december", 12),
        ("dec", 12),
        ("dez", 12),
        ("dezember", 12),
    ];

    // Check for month without a day number (whole month range)
    let has_day = Regex::new(r"\d").unwrap().is_match(&text_lower);
    if !has_day {
        for (name, month_num) in months {
            let pattern = format!(r"(?i)\b{}\b", regex::escape(name));
            if Regex::new(&pattern).unwrap().is_match(&text_lower) {
                let mut year = now.year();
                if *month_num < now.month() {
                    year += 1;
                }
                let start = NaiveDate::from_ymd_opt(year, *month_num, 1).unwrap();
                let end = if *month_num == 12 {
                    NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap() - ChronoDuration::days(1)
                } else {
                    NaiveDate::from_ymd_opt(year, *month_num + 1, 1).unwrap()
                        - ChronoDuration::days(1)
                };
                return (
                    start.format("%Y-%m-%d").to_string(),
                    end.format("%Y-%m-%d").to_string(),
                    format!("{} {}", name, year),
                );
            }
        }
    }

    // 5. Try single-date parser for everything else
    if let Some((date, description)) = parse_single_date(text) {
        return (
            date.format("%Y-%m-%d").to_string(),
            date.format("%Y-%m-%d").to_string(),
            description,
        );
    }

    // 6. Default: today
    (
        today.format("%Y-%m-%d").to_string(),
        today.format("%Y-%m-%d").to_string(),
        "today".to_string(),
    )
}

/// Parse a natural language date expression and return a single date.
/// This is an alias for parse_single_date for backward compatibility.
fn parse_date_expr(text: &str) -> Option<(NaiveDate, String)> {
    parse_single_date(text)
}

/// Parse a natural language datetime expression for scheduling.
///
/// Supports formats like:
/// - "tomorrow 9am", "friday 14:00"
/// - "2026-01-20 10:30", "2026/01/20 10:30"
/// - "jan 20 9am", "20 jan 14:00"
/// - Relative: "in 2 hours", "in 30 minutes"
///
/// Returns an ISO datetime string suitable for the schedule_at parameter.
fn parse_schedule_datetime(text: &str) -> Result<String> {
    use chrono::{Datelike, Weekday};
    use chrono_tz::Europe::Berlin;
    use regex::Regex;

    let now = Local::now();
    let text_lower = text.to_lowercase().trim().to_string();

    // Default time if not specified
    let mut hour = 9;
    let mut minute = 0;
    let mut date = now.date_naive();

    // Extract time patterns first
    // Pattern: "14:00", "9:30", "9am", "2pm", "14 uhr"
    let time_24h_re = Regex::new(r"(\d{1,2}):(\d{2})").unwrap();
    let time_ampm_re = Regex::new(r"(\d{1,2})(?::(\d{2}))?\s*(am|pm)").unwrap();
    let time_uhr_re = Regex::new(r"(\d{1,2})\s*uhr").unwrap();

    let mut remaining = text_lower.clone();

    if let Some(caps) = time_24h_re.captures(&text_lower) {
        hour = caps.get(1).unwrap().as_str().parse().unwrap_or(9);
        minute = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
        remaining = time_24h_re.replace(&remaining, "").to_string();
    } else if let Some(caps) = time_ampm_re.captures(&text_lower) {
        hour = caps.get(1).unwrap().as_str().parse().unwrap_or(9);
        minute = caps.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
        let ampm = caps.get(3).unwrap().as_str();
        if ampm == "pm" && hour != 12 {
            hour += 12;
        } else if ampm == "am" && hour == 12 {
            hour = 0;
        }
        remaining = time_ampm_re.replace(&remaining, "").to_string();
    } else if let Some(caps) = time_uhr_re.captures(&text_lower) {
        hour = caps.get(1).unwrap().as_str().parse().unwrap_or(9);
        remaining = time_uhr_re.replace(&remaining, "").to_string();
    }

    let remaining = remaining.trim();

    // Check for "in X hours/minutes" pattern
    let in_duration_re = Regex::new(r"in\s+(\d+)\s*(h|hr|hrs|hours?|m|min|mins|minutes?)").unwrap();
    if let Some(caps) = in_duration_re.captures(remaining) {
        let value: i64 = caps.get(1).unwrap().as_str().parse().unwrap_or(1);
        let unit = caps.get(2).unwrap().as_str();
        let duration = if unit.starts_with('h') {
            ChronoDuration::hours(value)
        } else {
            ChronoDuration::minutes(value)
        };
        let scheduled = now + duration;
        return Ok(scheduled.with_timezone(&Berlin).to_rfc3339());
    }

    // Relative days
    let relative_days: &[(&str, i64)] = &[
        ("today", 0),
        ("heute", 0),
        ("tomorrow", 1),
        ("morgen", 1),
    ];
    for (keyword, offset) in relative_days {
        if remaining.contains(keyword) {
            date = now.date_naive() + ChronoDuration::days(*offset);
            break;
        }
    }

    // Weekday names (next occurrence)
    let weekdays: &[(&str, Weekday)] = &[
        ("monday", Weekday::Mon), ("mon", Weekday::Mon), ("montag", Weekday::Mon),
        ("tuesday", Weekday::Tue), ("tue", Weekday::Tue), ("dienstag", Weekday::Tue),
        ("wednesday", Weekday::Wed), ("wed", Weekday::Wed), ("mittwoch", Weekday::Wed),
        ("thursday", Weekday::Thu), ("thu", Weekday::Thu), ("donnerstag", Weekday::Thu),
        ("friday", Weekday::Fri), ("fri", Weekday::Fri), ("freitag", Weekday::Fri),
        ("saturday", Weekday::Sat), ("sat", Weekday::Sat), ("samstag", Weekday::Sat),
        ("sunday", Weekday::Sun), ("sun", Weekday::Sun), ("sonntag", Weekday::Sun),
    ];
    for (name, weekday) in weekdays {
        if remaining.contains(name) {
            let today = now.date_naive();
            let today_weekday = today.weekday();
            let mut days_ahead = (*weekday as i64 - today_weekday as i64 + 7) % 7;
            if days_ahead == 0 {
                days_ahead = 7; // Next week if today
            }
            date = today + ChronoDuration::days(days_ahead);
            break;
        }
    }

    // ISO date: 2026-01-20 or 2026/01/20
    let iso_date_re = Regex::new(r"(\d{4})[-/](\d{1,2})[-/](\d{1,2})").unwrap();
    if let Some(caps) = iso_date_re.captures(remaining) {
        let year: i32 = caps.get(1).unwrap().as_str().parse().unwrap_or(now.year());
        let month: u32 = caps.get(2).unwrap().as_str().parse().unwrap_or(1);
        let day: u32 = caps.get(3).unwrap().as_str().parse().unwrap_or(1);
        if let Some(d) = NaiveDate::from_ymd_opt(year, month, day) {
            date = d;
        }
    }

    // Month names: "jan 20", "20 jan", "jan 20 2026"
    let months: &[(&str, u32)] = &[
        ("january", 1), ("jan", 1), ("februar", 2), ("feb", 2),
        ("march", 3), ("mar", 3), ("april", 4), ("apr", 4),
        ("may", 5), ("mai", 5), ("june", 6), ("jun", 6),
        ("july", 7), ("jul", 7), ("august", 8), ("aug", 8),
        ("september", 9), ("sep", 9), ("october", 10), ("oct", 10),
        ("november", 11), ("nov", 11), ("december", 12), ("dec", 12),
    ];
    for (month_name, month_num) in months {
        if remaining.contains(month_name) {
            let day_re = Regex::new(r"(\d{1,2})").unwrap();
            let year_re = Regex::new(r"(\d{4})").unwrap();
            let day: u32 = day_re.captures(remaining)
                .and_then(|c| c.get(1).map(|m| m.as_str().parse().unwrap_or(1)))
                .unwrap_or(1);
            let year: i32 = year_re.captures(remaining)
                .and_then(|c| c.get(1).map(|m| m.as_str().parse().unwrap_or(now.year())))
                .unwrap_or_else(|| {
                    // Use current or next year
                    if *month_num < now.month() || (*month_num == now.month() && day < now.day()) {
                        now.year() + 1
                    } else {
                        now.year()
                    }
                });
            if let Some(d) = NaiveDate::from_ymd_opt(year, *month_num, day) {
                date = d;
            }
            break;
        }
    }

    // Build the final datetime
    let scheduled = date.and_hms_opt(hour, minute, 0)
        .ok_or_else(|| anyhow!("invalid time: {}:{}", hour, minute))?;

    // Convert to timezone-aware
    let scheduled_tz = Berlin.from_local_datetime(&scheduled)
        .single()
        .ok_or_else(|| anyhow!("ambiguous or invalid datetime"))?;

    // Validate it's in the future
    if scheduled_tz <= now.with_timezone(&Berlin) {
        return Err(anyhow!("scheduled time must be in the future"));
    }

    Ok(scheduled_tz.to_rfc3339())
}

fn handle_mail(ctx: &RuntimeContext, cmd: MailCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ctx.service_client()?;

    match cmd {
        MailCommand::List(args) => handle_mail_list(ctx, &client, &account, args),
        MailCommand::Search(args) => handle_mail_search(ctx, &client, &account, args),
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
        MailCommand::EmptyFolder(args) => handle_mail_empty_folder(ctx, &client, &account, args),
        MailCommand::Spam(args) => handle_mail_spam(ctx, &client, &account, args),
    }
}

fn handle_mail_list(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailListArgs,
) -> Result<()> {
    // Parse date filter if provided
    let date_filter = if !args.when.is_empty() {
        let when_text = args.when.join(" ");
        parse_date_expr(&when_text)
    } else {
        None
    };

    // Print date header if filtering
    if let Some((filter_date, ref description)) = date_filter {
        if !ctx.common.json && !ctx.common.yaml && !ctx.common.quiet {
            println!("Messages from {}:\n", description);
        }
        // Use filter_date below
        let _ = filter_date;
    }

    // Try to list from local database first (sorted by date), fall back to server
    let db_path = ctx.paths.sync_db_path(account);

    if db_path.exists() {
        let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
        let mail_dir = get_mail_dir(ctx, account)?;

        // Get messages from database, already sorted by received_at DESC
        // Request more than limit to account for filtering (more if date filtering)
        let fetch_limit = if date_filter.is_some() {
            args.limit * 10 // Fetch more when filtering by date
        } else {
            args.limit * 2
        };
        let db_messages = db
            .list_messages(&args.folder, fetch_limit)
            .map_err(|e| anyhow!("{e}"))?;

        let mut output: Vec<serde_json::Value> = Vec::new();
        for db_msg in db_messages {
            // Filter unread if requested
            if args.unread && db_msg.is_read {
                continue;
            }

            // Filter by date if requested
            if let Some((filter_date, _)) = date_filter {
                if let Some(ref received_at) = db_msg.received_at {
                    // Parse the received_at timestamp and compare dates
                    if let Ok(msg_dt) = DateTime::parse_from_rfc3339(received_at) {
                        let msg_date = msg_dt.date_naive();
                        if msg_date != filter_date {
                            continue;
                        }
                    } else if let Ok(msg_dt) = NaiveDateTime::parse_from_str(received_at, "%Y-%m-%dT%H:%M:%S") {
                        if msg_dt.date() != filter_date {
                            continue;
                        }
                    } else {
                        // Can't parse date, skip
                        continue;
                    }
                } else {
                    // No date, skip when filtering
                    continue;
                }
            }

            // Get flags from Maildir if available
            let (is_read, is_flagged) =
                if let Ok(Some(maildir_msg)) = mail_dir.get(&args.folder, &db_msg.local_id) {
                    (maildir_msg.flags.seen, maildir_msg.flags.flagged)
                } else {
                    (db_msg.is_read, false)
                };

            output.push(serde_json::json!({
                "id": db_msg.local_id,
                "subject": db_msg.subject.unwrap_or_else(|| "(no subject)".to_string()),
                "from": db_msg.from_addr.unwrap_or_else(|| "unknown".to_string()),
                "date": db_msg.received_at.unwrap_or_default(),
                "is_read": is_read,
                "is_flagged": is_flagged,
                "folder": db_msg.folder,
            }));

            if output.len() >= args.limit {
                break;
            }
        }

        emit_output(&ctx.common, &output)?;
    } else {
        // Fall back to server (date filtering not supported for server-side)
        if date_filter.is_some() {
            return Err(anyhow!("Date filtering requires synced messages. Run 'h8 mail sync' first."));
        }
        let messages = client
            .mail_list(account, &args.folder, args.limit, args.unread)
            .map_err(|e| anyhow!("{e}"))?;
        emit_output(&ctx.common, &messages)?;
    }

    Ok(())
}

fn handle_mail_search(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailSearchArgs,
) -> Result<()> {
    let messages = client
        .mail_search(account, &args.query, &args.folder, args.limit)
        .map_err(|e| anyhow!("{e}"))?;

    if !ctx.common.json && !ctx.common.yaml {
        let count = messages.as_array().map(|a| a.len()).unwrap_or(0);
        println!(
            "Found {} message(s) matching \"{}\" in {}:\n",
            count, args.query, args.folder
        );
    }

    emit_output(&ctx.common, &messages)?;
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

    let raw_content = msg.read_content().map_err(|e| anyhow!("{e}"))?;

    // Parse headers and body
    let (headers, body) = parse_email_content(&raw_content);

    // Convert HTML to plain text if needed (unless --raw is specified)
    let display_body = if args.raw {
        body.to_string()
    } else {
        convert_body_to_text(body)
    };

    // Reconstruct the display content
    let content = format!("{}\n{}", headers, display_body);

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

/// Parse email content into headers and body.
fn parse_email_content(content: &str) -> (&str, &str) {
    // Email headers and body are separated by a blank line (\r\n\r\n or \n\n)
    if let Some(pos) = content.find("\r\n\r\n") {
        let (headers, rest) = content.split_at(pos);
        (headers, &rest[4..]) // Skip the \r\n\r\n
    } else if let Some(pos) = content.find("\n\n") {
        let (headers, rest) = content.split_at(pos);
        (headers, &rest[2..]) // Skip the \n\n
    } else {
        // No body found, treat entire content as body
        ("", content)
    }
}

/// Convert email body to plain text, handling HTML if present.
fn convert_body_to_text(body: &str) -> String {
    let trimmed = body.trim();

    // Check if body looks like HTML
    if trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<HTML")
        || (trimmed.contains("<body") || trimmed.contains("<BODY"))
    {
        // Get terminal width for wrapping, default to 80
        let width = terminal_size::terminal_size()
            .map(|(w, _)| w.0 as usize)
            .unwrap_or(80);

        h8_core::html_to_text(body, width)
    } else {
        // Already plain text
        body.to_string()
    }
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
    // Parse schedule time if provided
    let schedule_at = if let Some(ref schedule_str) = args.schedule {
        Some(parse_schedule_datetime(schedule_str)?)
    } else {
        None
    };

    if args.all {
        // Send all drafts
        let mail_dir = get_mail_dir(ctx, account)?;
        let drafts = mail_dir.list(FOLDER_DRAFTS).map_err(|e| anyhow!("{e}"))?;

        for draft in drafts {
            send_draft(ctx, client, account, &mail_dir, &draft.id, schedule_at.as_deref())?;
        }
        return Ok(());
    }

    if let Some(id) = args.id {
        // Send specific draft
        let mail_dir = get_mail_dir(ctx, account)?;
        send_draft(ctx, client, account, &mail_dir, &id, schedule_at.as_deref())?;
    } else if !args.to.is_empty() {
        // Direct composition mode (for agents/programmatic use)
        let subject = args.subject.unwrap_or_default();
        let body = if args.body.as_deref() == Some("-") {
            // Read body from stdin
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            buf
        } else {
            args.body.unwrap_or_default()
        };

        let mut payload = serde_json::json!({
            "to": args.to,
            "cc": args.cc,
            "bcc": args.bcc,
            "subject": subject,
            "body": body,
            "html": args.html,
        });

        if let Some(ref schedule) = schedule_at {
            payload["schedule_at"] = serde_json::Value::String(schedule.clone());
        }

        let result = client
            .mail_send(account, payload)
            .map_err(|e| anyhow!("{e}"))?;

        // User-friendly output
        if !ctx.common.json && !ctx.common.yaml {
            if schedule_at.is_some() {
                println!("Scheduled: {}", subject);
            } else {
                println!("Sent: {}", subject);
            }
            println!("  To: {}", args.to.join(", "));
        }
        emit_output(&ctx.common, &result)?;
    } else if args.file.is_some() {
        // Read from file
        let mut payload = read_json_payload(args.file.as_ref())?;
        if let Some(ref schedule) = schedule_at {
            payload["schedule_at"] = serde_json::Value::String(schedule.clone());
        }
        let result = client
            .mail_send(account, payload)
            .map_err(|e| anyhow!("{e}"))?;
        emit_output(&ctx.common, &result)?;
    } else {
        return Err(anyhow!(
            "No email to send. Use --to/--subject/--body, --file, or provide a draft ID."
        ));
    }

    Ok(())
}

fn send_draft(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    mail_dir: &Maildir,
    draft_id: &str,
    schedule_at: Option<&str>,
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
    let mut payload = serde_json::json!({
        "to": doc.to,
        "cc": doc.cc,
        "bcc": doc.bcc,
        "subject": doc.subject,
        "body": doc.body,
        "html": false,
    });

    // Add schedule time if provided
    if let Some(schedule) = schedule_at {
        payload["schedule_at"] = serde_json::Value::String(schedule.to_string());
    }

    // Send via service
    let result = client
        .mail_send(account, payload)
        .map_err(|e| anyhow!("{e}"))?;

    // Delete local draft on success
    mail_dir
        .delete(FOLDER_DRAFTS, draft_id)
        .map_err(|e| anyhow!("{e}"))?;

    if schedule_at.is_some() {
        println!("Scheduled: {}", draft_id);
    } else {
        println!("Sent: {}", draft_id);
    }
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

    open_editor_and_save_draft(ctx, account, doc, !args.no_edit, true)
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

    open_editor_and_save_draft(ctx, account, doc, true, false)
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

    // Forward needs to show empty to/cc/bcc since recipient is not yet specified
    open_editor_and_save_draft(ctx, account, doc, true, true)
}

/// Parse message IDs from command args, handling comma-separated values.
/// e.g., ["id1", "id2,id3", "id4"] -> ["id1", "id2", "id3", "id4"]
fn parse_message_ids(ids: &[String]) -> Vec<String> {
    ids.iter()
        .flat_map(|s| s.split(','))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Parse an email address string like "Name <email@x.com>" or "email@x.com".
/// Returns (email, Option<name>).
fn parse_email_address(s: &str) -> Option<(String, Option<String>)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Check for "Name <email>" format
    if let Some(start) = s.find('<') {
        if let Some(end) = s.find('>') {
            let email = s[start + 1..end].trim().to_lowercase();
            let name = s[..start].trim();
            let name = if name.is_empty() {
                None
            } else {
                Some(name.trim_matches('"').to_string())
            };
            if email.contains('@') {
                return Some((email, name));
            }
        }
    }

    // Just an email address
    if s.contains('@') {
        return Some((s.to_lowercase(), None));
    }

    None
}

/// Parse move args to extract target folder from positional args or --to flag.
/// Supports: "h8 mail move id1 id2 --to folder" or "h8 mail move id1 id2 to folder"
fn parse_move_args(args: &MailMoveArgs) -> Result<(Vec<String>, String)> {
    let all_args = parse_message_ids(&args.ids);

    // If --to is provided, use it
    if let Some(ref target) = args.target {
        return Ok((all_args, target.clone()));
    }

    // Otherwise, look for natural "to" keyword in positional args
    let mut ids = Vec::new();
    let mut target: Option<String> = None;
    let mut found_to = false;

    for arg in all_args {
        if found_to {
            // Everything after "to" is the target folder
            if target.is_some() {
                return Err(anyhow!("multiple target folders specified"));
            }
            target = Some(arg);
        } else if arg.eq_ignore_ascii_case("to") {
            found_to = true;
        } else {
            ids.push(arg);
        }
    }

    match target {
        Some(t) => Ok((ids, t)),
        None => Err(anyhow!(
            "target folder required: use --to <folder> or 'to <folder>'"
        )),
    }
}

fn handle_mail_move(ctx: &RuntimeContext, account: &str, args: MailMoveArgs) -> Result<()> {
    let service = ctx.service_client()?;

    // Get IDs either from args or from search query
    let (ids, target) = if let Some(ref query) = args.query {
        // Search for messages matching query
        let results = service
            .mail_search(account, query, &args.folder, args.limit)
            .map_err(|e| anyhow!("{e}"))?;

        let search_ids: Vec<String> = results
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if search_ids.is_empty() {
            println!("No messages found matching: {}", query);
            return Ok(());
        }

        // Get target from --to flag (required when using --query)
        let target = args.target.clone().ok_or_else(|| {
            anyhow!("--to <folder> is required when using --query")
        })?;

        // Show what will be moved
        println!("Found {} message(s) matching \"{}\":", search_ids.len(), query);
        for (i, id) in search_ids.iter().take(10).enumerate() {
            if let Some(msg) = results.as_array().and_then(|a| a.get(i)) {
                let subject = msg.get("subject").and_then(|v| v.as_str()).unwrap_or("(no subject)");
                let from = msg.get("from").and_then(|v| v.as_str()).unwrap_or("unknown");
                println!("  {} - {} ({})", id, subject, from);
            }
        }
        if search_ids.len() > 10 {
            println!("  ... and {} more", search_ids.len() - 10);
        }

        if args.dry_run {
            println!("\nDry run - no messages moved");
            return Ok(());
        }

        println!();
        (search_ids, target)
    } else {
        let (ids, target) = parse_move_args(&args)?;
        if ids.is_empty() {
            return Err(anyhow!("no message IDs provided (use IDs or --query)"));
        }
        if args.dry_run {
            println!("Would move {} message(s) to {}", ids.len(), target);
            return Ok(());
        }
        (ids, target)
    };

    let mail_dir = get_mail_dir(ctx, account)?;
    let db_path = ctx.paths.sync_db_path(account);
    let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;

    let mut moved_count = 0;
    let mut errors: Vec<String> = Vec::new();

    for id in &ids {
        // Get remote_id for server sync
        let remote_id = db
            .get_message(id)
            .ok()
            .flatten()
            .map(|m| m.remote_id.clone());

        // Sync to server first if enabled and we have remote_id
        if args.sync {
            if let Some(ref rid) = remote_id {
                match service.mail_move(account, &args.folder, rid, &target, args.create) {
                    Ok(resp) => {
                        if resp.get("success").and_then(|v| v.as_bool()) != Some(true) {
                            let err = resp
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown error");
                            errors.push(format!("{}: server error: {}", id, err));
                            continue;
                        }
                    }
                    Err(e) => {
                        errors.push(format!("{}: server sync failed: {}", id, e));
                        continue;
                    }
                }
            }
        }

        // Move locally
        match mail_dir.move_to(&args.folder, id, &target) {
            Ok(Some(_)) => {
                // Update database folder
                if let Some(mut msg) = db.get_message(id).ok().flatten() {
                    msg.folder = target.clone();
                    let _ = db.upsert_message(&msg);
                }
                if !ctx.common.quiet {
                    println!("Moved {} to {}", id, target);
                }
                moved_count += 1;
            }
            Ok(None) => {
                errors.push(format!("message not found locally: {}", id));
            }
            Err(e) => {
                errors.push(format!("{}: {}", id, e));
            }
        }
    }

    // Summary for multiple moves
    if ids.len() > 1 && !ctx.common.quiet {
        println!("\n{} of {} messages moved to {}", moved_count, ids.len(), target);
    }

    // Report errors
    if !errors.is_empty() {
        for err in &errors {
            eprintln!("Error: {}", err);
        }
        if moved_count == 0 {
            return Err(anyhow!("no messages were moved"));
        }
    }

    Ok(())
}

fn handle_mail_delete(ctx: &RuntimeContext, account: &str, args: MailDeleteArgs) -> Result<()> {
    let ids = parse_message_ids(&args.ids);

    if ids.is_empty() {
        return Err(anyhow!("no message IDs provided"));
    }

    let mail_dir = get_mail_dir(ctx, account)?;
    let db_path = ctx.paths.sync_db_path(account);
    let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
    let service = ctx.service_client()?;

    let mut deleted_count = 0;
    let mut errors: Vec<String> = Vec::new();

    for id in &ids {
        // Get remote_id for server sync
        let remote_id = db
            .get_message(id)
            .ok()
            .flatten()
            .map(|m| m.remote_id.clone());

        // Sync deletion to server first if enabled and we have remote_id
        if args.sync {
            if let Some(ref rid) = remote_id {
                match service.mail_delete(account, &args.folder, rid, args.force) {
                    Ok(resp) => {
                        if resp.get("success").and_then(|v| v.as_bool()) != Some(true) {
                            let err = resp
                                .get("error")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown error");
                            errors.push(format!("{}: server error: {}", id, err));
                            continue;
                        }
                    }
                    Err(e) => {
                        errors.push(format!("{}: server sync failed: {}", id, e));
                        continue;
                    }
                }
            }
        }

        // Delete/move locally
        if args.force {
            // Permanently delete locally
            match mail_dir.delete(&args.folder, id) {
                Ok(true) => {
                    // Also delete from database
                    let _ = db.delete_message(id);
                    if !ctx.common.quiet {
                        println!("Deleted {}", id);
                    }
                    deleted_count += 1;
                }
                Ok(false) => {
                    // Server deletion succeeded, local file may already be gone
                    let _ = db.delete_message(id);
                    if !ctx.common.quiet {
                        println!("Deleted {}", id);
                    }
                    deleted_count += 1;
                }
                Err(e) => {
                    errors.push(format!("{}: {}", id, e));
                }
            }
        } else {
            // Move to trash locally
            match mail_dir.move_to(&args.folder, id, FOLDER_TRASH) {
                Ok(Some(_)) => {
                    // Update database folder
                    if let Some(mut msg) = db.get_message(id).ok().flatten() {
                        msg.folder = FOLDER_TRASH.to_string();
                        let _ = db.upsert_message(&msg);
                    }
                    if !ctx.common.quiet {
                        println!("Moved {} to trash", id);
                    }
                    deleted_count += 1;
                }
                Ok(None) => {
                    // Server deletion succeeded, local file may already be gone
                    let _ = db.delete_message(id);
                    if !ctx.common.quiet {
                        println!("Moved {} to trash", id);
                    }
                    deleted_count += 1;
                }
                Err(e) => {
                    errors.push(format!("{}: {}", id, e));
                }
            }
        }
    }

    // Summary for multiple deletions
    if ids.len() > 1 && !ctx.common.quiet {
        let action = if args.force { "deleted" } else { "moved to trash" };
        println!("\n{} of {} messages {}", deleted_count, ids.len(), action);
    }

    // Report errors
    if !errors.is_empty() {
        for err in &errors {
            eprintln!("Error: {}", err);
        }
        if deleted_count == 0 {
            return Err(anyhow!("no messages were deleted"));
        }
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

    open_editor_and_save_draft(ctx, account, doc, true, false)
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

        // Batch fetch all messages in chunks for efficiency
        const BATCH_SIZE: usize = 50;
        let mut synced = 0;
        let mut failed = 0;

        for chunk in to_sync.chunks(BATCH_SIZE) {
            // Collect IDs for batch request
            let ids: Vec<&str> = chunk.iter().map(|(id, _)| *id).collect();

            print!("\r  [{}/{}] Fetching batch...    ", synced + 1, total);
            let _ = io::stdout().flush();

            // Batch fetch all messages in this chunk
            let batch_result = client.mail_batch_get(account, folder, &ids);
            let batch_messages: Vec<Option<Value>> = match batch_result {
                Ok(val) => {
                    if let Some(arr) = val.as_array() {
                        arr.iter()
                            .map(|v| if v.is_null() { None } else { Some(v.clone()) })
                            .collect()
                    } else {
                        // Fallback: treat as empty
                        vec![None; ids.len()]
                    }
                }
                Err(e) => {
                    log::warn!(
                        "Batch fetch failed: {}, falling back to individual fetches",
                        e
                    );
                    // Fall back to individual fetches
                    ids.iter()
                        .map(|id| client.mail_get(account, folder, id).ok())
                        .collect()
                }
            };

            // Process each message from the batch
            for ((remote_id, msg_val), full_msg_opt) in chunk.iter().zip(batch_messages.into_iter())
            {
                // Progress indicator
                print!("\r  [{}/{}] Syncing...    ", synced + 1, total);
                let _ = io::stdout().flush();

                let full_msg = match full_msg_opt {
                    Some(msg) => msg,
                    None => {
                        log::warn!("Failed to fetch message {}", remote_id);
                        failed += 1;
                        continue;
                    }
                };

                // Allocate human-readable ID
                let local_id = id_gen.allocate(remote_id).map_err(|e| anyhow!("{e}"))?;

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

                // Cache email addresses for autocomplete
                // Parse "Name <email>" or just "email" format
                if let Some((email, name)) = parse_email_address(from) {
                    if folder == "sent" {
                        let _ = db.record_sent_address(&email, name.as_deref());
                    } else {
                        let _ = db.record_received_address(&email, name.as_deref());
                    }
                }

                synced += 1;
            }
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

fn handle_mail_empty_folder(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailEmptyFolderArgs,
) -> Result<()> {
    // Confirm unless --yes is passed
    if !args.yes {
        print!(
            "Permanently delete all items in '{}'? This cannot be undone. [y/N] ",
            args.folder
        );
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let result = client
        .mail_empty_folder(account, &args.folder)
        .map_err(|e| anyhow!("{e}"))?;

    if let Some(count) = result.get("deleted_count").and_then(|v| v.as_u64()) {
        if count == 0 {
            println!("Folder '{}' is already empty", args.folder);
        } else {
            println!(
                "Permanently deleted {} item(s) from '{}'",
                count, args.folder
            );
        }
    } else if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
        eprintln!("Error: {}", err);
    }

    emit_output(&ctx.common, &result)?;
    Ok(())
}

fn handle_mail_spam(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailSpamArgs,
) -> Result<()> {
    // Parse message IDs (support comma-separated)
    let ids = parse_message_ids(&args.ids);

    if ids.is_empty() {
        eprintln!("No message IDs provided");
        return Ok(());
    }

    // Resolve human-readable IDs to remote IDs
    let db_path = ctx.paths.sync_db_path(account);
    let db = if db_path.exists() {
        Some(Database::open(&db_path).map_err(|e| anyhow!("{e}"))?)
    } else {
        None
    };
    let id_gen = db.as_ref().map(IdGenerator::new);

    let is_spam = !args.not_spam;
    let move_item = !args.no_move;
    let mut success_count = 0;
    let mut errors: Vec<String> = Vec::new();

    for id in &ids {
        // Resolve ID
        let remote_id = if let Some(ref id_generator) = id_gen {
            id_generator
                .resolve(id)
                .map_err(|e| anyhow!("{e}"))?
                .unwrap_or_else(|| id.clone())
        } else {
            id.clone()
        };

        match client.mail_mark_spam(account, &remote_id, is_spam, move_item) {
            Ok(result) => {
                if result.get("success").and_then(|v| v.as_bool()) == Some(true) {
                    success_count += 1;
                    if is_spam {
                        if move_item {
                            println!("Marked {} as spam (moved to junk)", id);
                        } else {
                            println!("Marked {} as spam", id);
                        }
                    } else if move_item {
                        println!("Marked {} as not spam (moved to inbox)", id);
                    } else {
                        println!("Marked {} as not spam", id);
                    }
                } else if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
                    errors.push(format!("{}: {}", id, err));
                }
            }
            Err(e) => {
                errors.push(format!("{}: {}", id, e));
            }
        }
    }

    if !errors.is_empty() {
        for err in &errors {
            eprintln!("Error: {}", err);
        }
    }

    if success_count > 0 {
        let action = if is_spam { "spam" } else { "not spam" };
        println!(
            "Marked {} message(s) as {}",
            success_count, action
        );
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
    is_new_compose: bool,
) -> Result<()> {
    // Use template format for new compose to show empty to/cc/bcc fields
    let content = if is_new_compose {
        doc.to_template().map_err(|e| anyhow!("{e}"))?
    } else {
        doc.to_string().map_err(|e| anyhow!("{e}"))?
    };

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
        ContactsCommand::Update(args) => {
            // Build update payload from provided args
            let mut updates = serde_json::Map::new();
            if let Some(v) = args.name {
                updates.insert("display_name".to_string(), serde_json::json!(v));
            }
            if let Some(v) = args.given_name {
                updates.insert("given_name".to_string(), serde_json::json!(v));
            }
            if let Some(v) = args.surname {
                updates.insert("surname".to_string(), serde_json::json!(v));
            }
            if let Some(v) = args.email {
                updates.insert("email".to_string(), serde_json::json!(v));
            }
            if let Some(v) = args.phone {
                updates.insert("phone".to_string(), serde_json::json!(v));
            }
            if let Some(v) = args.company {
                updates.insert("company".to_string(), serde_json::json!(v));
            }
            if let Some(v) = args.job_title {
                updates.insert("job_title".to_string(), serde_json::json!(v));
            }

            if updates.is_empty() {
                return Err(anyhow!("no fields to update - specify at least one of: --name, --email, --phone, --company, --job-title, --given-name, --surname"));
            }

            let result = client
                .contacts_update(&account, &args.id, serde_json::Value::Object(updates))
                .map_err(|e| anyhow!("{e}"))?;

            if !ctx.common.json && !ctx.common.yaml {
                if result.get("success") == Some(&serde_json::json!(false)) {
                    if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
                        return Err(anyhow!("{}", err));
                    }
                } else {
                    let name = result.get("display_name").and_then(|v| v.as_str()).unwrap_or("Contact");
                    println!("Updated: {}", name);
                }
            }
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

fn handle_addr(ctx: &RuntimeContext, args: AddrArgs) -> Result<()> {
    let account = effective_account(ctx);
    let db_path = ctx.paths.sync_db_path(&account);

    if !db_path.exists() {
        return Err(anyhow!("no address cache yet - run 'h8 mail sync' first"));
    }

    let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;

    let addresses = if args.frequent || args.query.is_none() {
        db.frequent_addresses(args.limit).map_err(|e| anyhow!("{e}"))?
    } else {
        db.search_addresses(args.query.as_deref().unwrap(), args.limit)
            .map_err(|e| anyhow!("{e}"))?
    };

    if addresses.is_empty() {
        if let Some(ref q) = args.query {
            println!("No addresses found matching \"{}\"", q);
        } else {
            println!("No addresses cached yet - run 'h8 mail sync' to populate");
        }
        return Ok(());
    }

    if ctx.common.json || ctx.common.yaml {
        emit_output(&ctx.common, &serde_json::to_value(&addresses)?)?;
    } else {
        for addr in &addresses {
            let name_part = addr.name.as_deref().unwrap_or("");
            let count_info = format!("sent:{} recv:{}", addr.send_count, addr.receive_count);
            if name_part.is_empty() {
                println!("{:<40} ({})", addr.email, count_info);
            } else {
                println!("{} <{}> ({})", name_part, addr.email, count_info);
            }
        }
    }

    Ok(())
}

fn handle_agenda(ctx: &RuntimeContext, args: AgendaArgs) -> Result<()> {
    let account = args.account.unwrap_or_else(|| effective_account(ctx));

    // Get view from args or config default
    let view = args
        .view
        .unwrap_or_else(|| ctx.config.calendar.default_view.into());

    // Parse timezone
    let tz = ctx
        .config
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::UTC);

    // Parse date from positional argument using unified parser
    let when_text = if args.when.is_empty() {
        "today".to_string()
    } else {
        args.when.join(" ")
    };

    let target_date = if let Some((date, _desc)) = parse_single_date(&when_text) {
        date
    } else {
        // Default to today if parsing fails
        Local::now().with_timezone(&tz).date_naive()
    };

    let start_str = target_date.format("%Y-%m-%d").to_string();
    let end_str = target_date.format("%Y-%m-%d").to_string();

    // Try local cache first for lightning-fast access
    let db_path = ctx.paths.sync_db_path(&account);
    let events_val = if db_path.exists() {
        let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;

        // Check if we have cached events for this date range
        let cached_events = db
            .list_calendar_events_range(&start_str, &end_str)
            .map_err(|e| anyhow!("{e}"))?;

        if !cached_events.is_empty() {
            // Convert cached events to JSON format matching server response
            let events_json: Vec<serde_json::Value> = cached_events
                .into_iter()
                .map(|e| {
                    serde_json::json!({
                        "id": e.local_id,
                        "remote_id": e.remote_id,
                        "subject": e.subject,
                        "location": e.location,
                        "start": e.start,
                        "end": e.end,
                        "is_all_day": e.is_all_day,
                    })
                })
                .collect();
            serde_json::json!(events_json)
        } else {
            // No cached events, fall back to server
            fetch_agenda_from_server(ctx, &account, &target_date)?
        }
    } else {
        // No cache, fetch from server
        fetch_agenda_from_server(ctx, &account, &target_date)?
    };

    if ctx.common.json || ctx.common.yaml || !io::stdout().is_terminal() {
        emit_output(&ctx.common, &events_val)?;
        return Ok(());
    }

    let events: Vec<AgendaItem> =
        serde_json::from_value(events_val.clone()).context("parsing agenda items")?;
    render_agenda(&events, tz, view, target_date)?;
    Ok(())
}

/// Fetch agenda events from server (fallback when no cache).
fn fetch_agenda_from_server(
    ctx: &RuntimeContext,
    account: &str,
    target_date: &NaiveDate,
) -> Result<Value> {
    let client = ctx.service_client()?;
    let start = target_date.and_hms_opt(0, 0, 0).unwrap();
    let end = target_date.and_hms_opt(23, 59, 59).unwrap();

    let events_val = client
        .calendar_list(
            account,
            1,
            Some(&start.format("%Y-%m-%dT%H:%M:%S").to_string()),
            Some(&end.format("%Y-%m-%dT%H:%M:%S").to_string()),
        )
        .map_err(|e| anyhow!("{e}"))?;

    // Sync to local cache
    let _ = sync_calendar_events(ctx, account, &events_val);

    Ok(events_val)
}

fn handle_free(ctx: &RuntimeContext, cmd: FreeCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ctx.service_client()?;
    let slots = client
        .free_slots(&account, cmd.weeks, cmd.duration, cmd.limit)
        .map_err(|e| anyhow!("{e}"))?;

    // Use JSON/YAML output if requested, otherwise render nicely
    if ctx.common.json || ctx.common.yaml || !io::stdout().is_terminal() {
        emit_output(&ctx.common, &slots)?;
    } else {
        let view = cmd
            .view
            .unwrap_or_else(|| ctx.config.calendar.default_view.into());
        render_free_slots(&slots, ctx, view)?;
    }
    Ok(())
}

fn handle_ppl(ctx: &RuntimeContext, cmd: PplCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ctx.service_client()?;

    match cmd {
        PplCommand::Agenda(args) => {
            let view = args
                .view
                .unwrap_or_else(|| ctx.config.calendar.default_view.into());
            // Resolve alias to email
            let person_email = ctx
                .config
                .resolve_person(&args.person)
                .map_err(|e| anyhow!("{e}"))?;
            let result = client
                .ppl_agenda(
                    &account,
                    &person_email,
                    args.days,
                    args.from_date.as_deref(),
                    args.to_date.as_deref(),
                )
                .map_err(|e| anyhow!("{e}"))?;

            // Use JSON/YAML output if requested, otherwise render nicely
            if ctx.common.json || ctx.common.yaml || !io::stdout().is_terminal() {
                emit_output(&ctx.common, &result)?;
            } else {
                let items: Vec<PplAgendaItem> =
                    serde_json::from_value(result).context("parsing ppl agenda items")?;
                // Use original alias for display, but email was used for lookup
                let display_name = if args.person != person_email {
                    format!("{} ({})", args.person, person_email)
                } else {
                    person_email
                };
                render_ppl_agenda(&display_name, &items, ctx, view)?;
            }
        }
        PplCommand::Free(args) => {
            let view = args
                .view
                .unwrap_or_else(|| ctx.config.calendar.default_view.into());
            // Resolve alias to email
            let person_email = ctx
                .config
                .resolve_person(&args.person)
                .map_err(|e| anyhow!("{e}"))?;
            let result = client
                .ppl_free(
                    &account,
                    &person_email,
                    args.weeks,
                    args.duration,
                    args.limit,
                )
                .map_err(|e| anyhow!("{e}"))?;

            if ctx.common.json || ctx.common.yaml || !io::stdout().is_terminal() {
                emit_output(&ctx.common, &result)?;
            } else {
                let display_name = if args.person != person_email {
                    format!("{} ({})", args.person, person_email)
                } else {
                    person_email
                };
                render_free_slots_for_person(&display_name, &result, ctx, view)?;
            }
        }
        PplCommand::Common(args) => {
            let view = args
                .view
                .unwrap_or_else(|| ctx.config.calendar.default_view.into());
            // Resolve all aliases to emails
            let mut resolved_emails: Vec<String> = Vec::new();
            let mut display_names: Vec<String> = Vec::new();
            for person in &args.people {
                let email = ctx
                    .config
                    .resolve_person(person)
                    .map_err(|e| anyhow!("{e}"))?;
                if person != &email {
                    display_names.push(person.clone());
                } else {
                    display_names.push(email.clone());
                }
                resolved_emails.push(email);
            }
            let email_refs: Vec<&str> = resolved_emails.iter().map(|s| s.as_str()).collect();
            let result = client
                .ppl_common(&account, &email_refs, args.weeks, args.duration, args.limit)
                .map_err(|e| anyhow!("{e}"))?;

            if ctx.common.json || ctx.common.yaml || !io::stdout().is_terminal() {
                emit_output(&ctx.common, &result)?;
            } else {
                let label = display_names.join(", ");
                render_free_slots_for_person(&label, &result, ctx, view)?;
            }
        }
    }
    Ok(())
}

/// Render another person's agenda in a human-readable format.
fn render_ppl_agenda(
    person: &str,
    items: &[PplAgendaItem],
    ctx: &RuntimeContext,
    _view: AgendaView,
) -> Result<()> {
    use owo_colors::OwoColorize;

    let tz = ctx
        .config
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::UTC);

    // Print header
    println!("Calendar for: {}", person.bold());
    println!("{}", "\u{2500}".repeat(50));

    if items.is_empty() {
        println!("(no events)");
        return Ok(());
    }

    // Group events by date
    let mut events_by_date: std::collections::BTreeMap<String, Vec<&PplAgendaItem>> =
        std::collections::BTreeMap::new();

    for item in items {
        let date_str = item
            .start
            .as_deref()
            .map(|s| {
                // Extract date part (YYYY-MM-DD)
                if s.len() >= 10 {
                    s[..10].to_string()
                } else {
                    s.to_string()
                }
            })
            .unwrap_or_else(|| "Unknown".to_string());
        events_by_date.entry(date_str).or_default().push(item);
    }

    let today = Local::now().with_timezone(&tz).date_naive();

    for (date_str, day_items) in &events_by_date {
        // Parse date for nice formatting
        let date_label = if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let weekday = date.format("%a").to_string();
            if date == today {
                format!("{} {} (Today)", weekday, date_str)
            } else if date == today.succ_opt().unwrap_or(today) {
                format!("{} {} (Tomorrow)", weekday, date_str)
            } else {
                format!("{} {}", weekday, date_str)
            }
        } else {
            date_str.clone()
        };

        println!();
        println!("{}", date_label.cyan().bold());

        // Sort items by start time
        let mut sorted_items: Vec<_> = day_items.iter().collect();
        sorted_items.sort_by(|a, b| a.start.cmp(&b.start));

        for item in sorted_items {
            let start_time = item
                .start
                .as_deref()
                .and_then(|s| extract_time(s))
                .unwrap_or_else(|| "??:??".to_string());
            let end_time = item
                .end
                .as_deref()
                .and_then(|s| extract_time(s))
                .unwrap_or_else(|| "??:??".to_string());

            let time_range = format!("{}-{}", start_time, end_time);

            // Check if it's an all-day event (times are 00:00)
            let is_all_day = start_time == "00:00"
                && (end_time == "00:00" || end_time == "23:59" || end_time == "24:00");

            let status = item.status.as_deref().unwrap_or("Busy");
            let status_icon = match status {
                "Free" => "\u{2610}",                  // Empty checkbox
                "Tentative" => "\u{25cb}",             // Circle
                "Busy" => "\u{2588}",                  // Full block
                "OOF" | "OutOfOffice" => "\u{2708}",   // Airplane
                "WorkingElsewhere" => "\u{1f3e0}",     // House (fallback to text)
                _ => "\u{2588}",                       // Default to busy block
            };

            // Subject or status as label
            let label = item
                .subject
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(status);

            if is_all_day {
                println!("  {} (all day)  {}", status_icon, label.dimmed());
            } else {
                println!("  {} {:<13} {}", status_icon, time_range, label);
            }

            // Show location if available
            if let Some(loc) = item.location.as_deref().filter(|s| !s.is_empty()) {
                println!("    {} {}", ICON_LOCATION, loc.dimmed());
            }
        }
    }

    println!();
    Ok(())
}

/// Free slot item from the service.
#[derive(Debug, Deserialize)]
struct FreeSlotItem {
    start: Option<String>,
    end: Option<String>,
    date: Option<String>,
    #[allow(dead_code)]
    day: Option<String>,
    duration_minutes: Option<i64>,
}

/// Render free slots in a human-readable format.
fn render_free_slots(slots: &Value, ctx: &RuntimeContext, _view: AgendaView) -> Result<()> {
    use owo_colors::OwoColorize;

    let tz = ctx
        .config
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::UTC);

    let items: Vec<FreeSlotItem> =
        serde_json::from_value(slots.clone()).context("parsing free slots")?;

    println!("{}", "Free Slots".bold());
    println!("{}", "\u{2500}".repeat(50));

    if items.is_empty() {
        println!("(no free slots found)");
        return Ok(());
    }

    // Group by date
    let mut slots_by_date: std::collections::BTreeMap<String, Vec<&FreeSlotItem>> =
        std::collections::BTreeMap::new();

    for item in &items {
        let date_str = item.date.clone().unwrap_or_else(|| {
            item.start
                .as_deref()
                .map(|s| {
                    if s.len() >= 10 {
                        s[..10].to_string()
                    } else {
                        s.to_string()
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string())
        });
        slots_by_date.entry(date_str).or_default().push(item);
    }

    let today = Local::now().with_timezone(&tz).date_naive();

    for (date_str, day_slots) in &slots_by_date {
        let date_label = if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let weekday = date.format("%a").to_string();
            if date == today {
                format!("{} {} (Today)", weekday, date_str)
            } else if date == today.succ_opt().unwrap_or(today) {
                format!("{} {} (Tomorrow)", weekday, date_str)
            } else {
                format!("{} {}", weekday, date_str)
            }
        } else {
            date_str.clone()
        };

        println!();
        println!("{}", date_label.cyan().bold());

        for slot in day_slots {
            let start_time = slot
                .start
                .as_deref()
                .and_then(|s| extract_time(s))
                .unwrap_or_else(|| "??:??".to_string());
            let end_time = slot
                .end
                .as_deref()
                .and_then(|s| extract_time(s))
                .unwrap_or_else(|| "??:??".to_string());

            let duration = slot.duration_minutes.unwrap_or(0);
            let duration_str = if duration >= 60 {
                let hours = duration / 60;
                let mins = duration % 60;
                if mins > 0 {
                    format!("{}h {}m", hours, mins)
                } else {
                    format!("{}h", hours)
                }
            } else {
                format!("{}m", duration)
            };

            println!(
                "  {} {}-{}  {}",
                "\u{2610}".green(), // Empty checkbox in green
                start_time,
                end_time,
                duration_str.dimmed()
            );
        }
    }

    println!();
    Ok(())
}

/// Render free slots for a person (ppl free / ppl common).
fn render_free_slots_for_person(
    label: &str,
    slots: &Value,
    ctx: &RuntimeContext,
    _view: AgendaView,
) -> Result<()> {
    use owo_colors::OwoColorize;

    let tz = ctx
        .config
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::UTC);

    let items: Vec<FreeSlotItem> =
        serde_json::from_value(slots.clone()).context("parsing free slots")?;

    println!("Free slots for: {}", label.bold());
    println!("{}", "\u{2500}".repeat(50));

    if items.is_empty() {
        println!("(no free slots found)");
        return Ok(());
    }

    // Group by date
    let mut slots_by_date: std::collections::BTreeMap<String, Vec<&FreeSlotItem>> =
        std::collections::BTreeMap::new();

    for item in &items {
        let date_str = item.date.clone().unwrap_or_else(|| {
            item.start
                .as_deref()
                .map(|s| {
                    if s.len() >= 10 {
                        s[..10].to_string()
                    } else {
                        s.to_string()
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string())
        });
        slots_by_date.entry(date_str).or_default().push(item);
    }

    let today = Local::now().with_timezone(&tz).date_naive();

    for (date_str, day_slots) in &slots_by_date {
        let date_label = if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let weekday = date.format("%a").to_string();
            if date == today {
                format!("{} {} (Today)", weekday, date_str)
            } else if date == today.succ_opt().unwrap_or(today) {
                format!("{} {} (Tomorrow)", weekday, date_str)
            } else {
                format!("{} {}", weekday, date_str)
            }
        } else {
            date_str.clone()
        };

        println!();
        println!("{}", date_label.cyan().bold());

        for slot in day_slots {
            let start_time = slot
                .start
                .as_deref()
                .and_then(|s| extract_time(s))
                .unwrap_or_else(|| "??:??".to_string());
            let end_time = slot
                .end
                .as_deref()
                .and_then(|s| extract_time(s))
                .unwrap_or_else(|| "??:??".to_string());

            let duration = slot.duration_minutes.unwrap_or(0);
            let duration_str = if duration >= 60 {
                let hours = duration / 60;
                let mins = duration % 60;
                if mins > 0 {
                    format!("{}h {}m", hours, mins)
                } else {
                    format!("{}h", hours)
                }
            } else {
                format!("{}m", duration)
            };

            println!(
                "  {} {}-{}  {}",
                "\u{2610}".green(), // Empty checkbox in green
                start_time,
                end_time,
                duration_str.dimmed()
            );
        }
    }

    println!();
    Ok(())
}

/// Extract time (HH:MM) from an ISO datetime string.
fn extract_time(dt_str: &str) -> Option<String> {
    // Format: 2025-12-19T15:00:00+01:00 or 2025-12-19T15:00:00
    if let Some(t_pos) = dt_str.find('T') {
        let time_part = &dt_str[t_pos + 1..];
        if time_part.len() >= 5 {
            return Some(time_part[..5].to_string());
        }
    }
    None
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
        // Check if this is a mail message (has "from" field) vs calendar event (has "start" field)
        let is_mail = obj.contains_key("from") && !obj.contains_key("start");
        if is_mail {
            // Mail message format
            let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("???");
            let from = obj
                .get("from")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let is_read = obj.get("is_read").and_then(|v| v.as_bool()).unwrap_or(true);

            // Format date human-readably
            let date_str = obj
                .get("date")
                .or_else(|| obj.get("datetime_received"))
                .and_then(|v| v.as_str())
                .map(|dt| format_date_human(dt))
                .unwrap_or_default();

            // Use colors when outputting to TTY
            use owo_colors::OwoColorize;

            let use_color = std::io::stdout().is_terminal();

            if is_read {
                if use_color {
                    println!("  {} - {} [{}]", subject, date_str.dimmed(), id.cyan());
                } else {
                    println!("  {} - {} [{}]", subject, date_str, id);
                }
            } else {
                if use_color {
                    println!(
                        "{} {} - {} [{}]",
                        "*".yellow().bold(),
                        subject.bold(),
                        date_str.dimmed(),
                        id.cyan()
                    );
                } else {
                    println!("* {} - {} [{}]", subject, date_str, id);
                }
            }
            if use_color {
                println!("  {}", from.dimmed());
            } else {
                println!("  {}", from);
            }
            println!();
        } else {
            // Calendar event format
            use owo_colors::OwoColorize;
            let use_color = std::io::stdout().is_terminal();

            let start = obj.get("start").and_then(|v| v.as_str()).unwrap_or("");
            let end = obj.get("end").and_then(|v| v.as_str()).unwrap_or("");
            let location = obj.get("location").and_then(|v| v.as_str()).unwrap_or("");
            let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");

            // Format start/end times
            let time_range = format_calendar_time_range(start, end);

            if use_color {
                println!("{} {} [{}]", time_range.cyan(), subject.bold(), id.dimmed());
            } else {
                println!("{} {} [{}]", time_range, subject, id);
            }

            if !location.is_empty() {
                if use_color {
                    println!("  {}", location.dimmed());
                } else {
                    println!("  {}", location);
                }
            }
            println!();
        }
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

/// Format an ISO date string to a human-readable format.
/// Shows "Today 14:30", "Yesterday 09:15", "Mon 14:30", or "Dec 5" for older dates.
fn format_date_human(iso_date: &str) -> String {
    use chrono::{DateTime, Datelike, Local};

    let parsed = DateTime::parse_from_rfc3339(iso_date)
        .or_else(|_| DateTime::parse_from_str(iso_date, "%Y-%m-%dT%H:%M:%S%z"))
        .map(|dt| dt.with_timezone(&Local));

    let dt = match parsed {
        Ok(dt) => dt,
        Err(_) => return iso_date.to_string(),
    };

    let now = Local::now();
    let today = now.date_naive();
    let date = dt.date_naive();
    let yesterday = today.pred_opt().unwrap_or(today);

    if date == today {
        format!("Today {}", dt.format("%H:%M"))
    } else if date == yesterday {
        format!("Yesterday {}", dt.format("%H:%M"))
    } else if (today - date).num_days() < 7 {
        // Within last week, show day name
        dt.format("%a %H:%M").to_string()
    } else if date.year() == today.year() {
        // Same year, show month and day
        dt.format("%b %-d").to_string()
    } else {
        // Different year
        dt.format("%b %-d %Y").to_string()
    }
}

/// Format calendar start/end times as a human-readable range.
/// Shows "Today 14:00-15:30" or "Mon Dec 11 14:00-15:30" or "Tomorrow (all day)"
fn format_calendar_time_range(start: &str, end: &str) -> String {
    use chrono::{DateTime, Datelike, Local, NaiveDate};

    let parse_dt = |s: &str| -> Option<DateTime<Local>> {
        DateTime::parse_from_rfc3339(s)
            .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%z"))
            .map(|dt| dt.with_timezone(&Local))
            .ok()
    };

    let parse_date =
        |s: &str| -> Option<NaiveDate> { NaiveDate::parse_from_str(s, "%Y-%m-%d").ok() };

    let now = Local::now();
    let today = now.date_naive();
    let yesterday = today.pred_opt().unwrap_or(today);
    let tomorrow = today.succ_opt().unwrap_or(today);

    let format_date_prefix = |date: NaiveDate| -> String {
        if date == today {
            "Today".to_string()
        } else if date == yesterday {
            "Yesterday".to_string()
        } else if date == tomorrow {
            "Tomorrow".to_string()
        } else if (date - today).num_days().abs() < 7 {
            date.format("%a").to_string()
        } else if date.year() == today.year() {
            date.format("%a %b %-d").to_string()
        } else {
            date.format("%a %b %-d %Y").to_string()
        }
    };

    // Try parsing as datetime first
    if let Some(start_dt) = parse_dt(start) {
        let end_dt = parse_dt(end);
        let start_date = start_dt.date_naive();
        let date_prefix = format_date_prefix(start_date);

        let start_time = start_dt.format("%H:%M");
        let end_time = end_dt
            .map(|dt| dt.format("%H:%M").to_string())
            .unwrap_or_default();

        if end_time.is_empty() {
            format!("{} {}", date_prefix, start_time)
        } else {
            format!("{} {}-{}", date_prefix, start_time, end_time)
        }
    } else if let Some(start_date) = parse_date(start) {
        // All-day event (date only, no time)
        let date_prefix = format_date_prefix(start_date);
        format!("{} (all day)", date_prefix)
    } else {
        // Fallback
        format!("{} - {}", start, end)
    }
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

/// Check if oama is installed and optionally install it.
fn ensure_oama() -> Result<()> {
    // Check if oama is already in PATH
    if which::which("oama").is_ok() {
        return Ok(());
    }

    eprintln!("oama not found in PATH, attempting to install...");

    let install_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("could not determine home directory"))?
        .join(".local")
        .join("bin");

    // Get latest version from GitHub
    let client = reqwest::blocking::Client::new();
    let release: Value = client
        .get("https://api.github.com/repos/pdobsan/oama/releases/latest")
        .header("User-Agent", "h8-cli")
        .send()
        .context("fetching oama release info")?
        .json()
        .context("parsing oama release info")?;

    let version = release["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow!("missing tag_name in release"))?;

    // Determine platform
    let (os, arch) = match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => ("Darwin", "arm64"),
        ("macos", "x86_64") => ("Darwin", "x86_64"),
        ("linux", "aarch64") => ("Linux", "aarch64"),
        ("linux", "x86_64") => ("Linux", "x86_64"),
        (os, arch) => return Err(anyhow!("unsupported platform: {}-{}", os, arch)),
    };

    let tarball_name = format!("oama-{}-{}-{}.tar.gz", version, os, arch);
    let download_url = format!(
        "https://github.com/pdobsan/oama/releases/download/{}/{}",
        version, tarball_name
    );

    eprintln!("Downloading oama {} from {}...", version, download_url);

    // Download tarball
    let response = client
        .get(&download_url)
        .send()
        .context("downloading oama tarball")?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "failed to download oama: HTTP {}",
            response.status()
        ));
    }

    let tarball_bytes = response.bytes().context("reading oama tarball")?;

    // Extract to temp dir
    let temp_dir = tempfile::tempdir().context("creating temp directory")?;
    let tarball_path = temp_dir.path().join(&tarball_name);
    fs::write(&tarball_path, &tarball_bytes).context("writing tarball")?;

    // Extract using tar command (simpler than using a tar crate)
    let status = ProcCommand::new("tar")
        .arg("-xzf")
        .arg(&tarball_path)
        .arg("-C")
        .arg(temp_dir.path())
        .status()
        .context("extracting oama tarball")?;

    if !status.success() {
        return Err(anyhow!("failed to extract oama tarball"));
    }

    // Find the oama binary in the extracted files
    let mut oama_binary = None;
    for entry in walkdir::WalkDir::new(temp_dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_name() == "oama" && entry.file_type().is_file() {
            oama_binary = Some(entry.path().to_path_buf());
            break;
        }
    }

    let oama_binary =
        oama_binary.ok_or_else(|| anyhow!("oama binary not found in extracted tarball"))?;

    // Create install directory and copy binary
    fs::create_dir_all(&install_dir)
        .with_context(|| format!("creating install directory {}", install_dir.display()))?;

    let install_path = install_dir.join("oama");
    fs::copy(&oama_binary, &install_path)
        .with_context(|| format!("copying oama to {}", install_path.display()))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&install_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&install_path, perms)?;
    }

    eprintln!("Installed oama to {}", install_path.display());

    // Verify it's now in PATH or warn user
    if which::which("oama").is_err() {
        eprintln!(
            "Warning: oama installed but {} is not in PATH. Add it to your PATH:",
            install_dir.display()
        );
        eprintln!("  export PATH=\"{}:$PATH\"", install_dir.display());
        // Update PATH for current process
        let path = env::var("PATH").unwrap_or_default();
        // SAFETY: We're only modifying PATH for the current process, which is safe
        // as long as no other threads are reading env vars simultaneously.
        // This runs early in service startup before any concurrent access.
        unsafe {
            env::set_var("PATH", format!("{}:{}", install_dir.display(), path));
        }
    }

    Ok(())
}

fn start_service(ctx: &RuntimeContext) -> Result<()> {
    // Ensure oama is installed before starting service
    ensure_oama()?;

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

/// Internal slot representation for agenda rendering.
struct AgendaSlot {
    subject: String,
    short_name: String,
    location: Option<String>,
    start_label: String,
    end_label: String,
    start_min: u32,
    end_min: u32,
    all_day: bool,
    status: EventStatus,
}

fn render_agenda(
    events: &[AgendaItem],
    tz: chrono_tz::Tz,
    view: AgendaView,
    target_date: NaiveDate,
) -> Result<()> {
    let today = Local::now().with_timezone(&tz).date_naive();
    let is_today = target_date == today;
    let start_naive = target_date.and_hms_opt(0, 0, 0).unwrap();
    let end_naive = today.and_hms_opt(23, 59, 59).unwrap();
    let day_start = tz
        .from_local_datetime(&start_naive)
        .single()
        .unwrap_or_else(|| tz.from_utc_datetime(&start_naive));
    let day_end = tz
        .from_local_datetime(&end_naive)
        .single()
        .unwrap_or_else(|| tz.from_utc_datetime(&end_naive));

    let mut slots = Vec::new();
    let mut all_day_events = Vec::new();

    for ev in events {
        let raw_subject = ev
            .subject
            .clone()
            .unwrap_or_else(|| "(no subject)".to_string());
        let status = EventStatus::from_subject(&raw_subject);

        // Clean subject: remove status prefixes
        let subject = clean_subject(&raw_subject);
        let short_name = truncate_str(&subject, 12);

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

        let slot = AgendaSlot {
            subject: subject.clone(),
            short_name,
            location: ev.location.clone(),
            start_label: start_dt.format("%H:%M").to_string(),
            end_label: end_dt.format("%H:%M").to_string(),
            start_min,
            end_min,
            all_day: is_all_day,
            status,
        };

        if is_all_day {
            all_day_events.push(slot);
        } else {
            slots.push(slot);
        }
    }

    // Print header
    let weekday = target_date.format("%a").to_string();
    println!(
        "{} {} \u{00b7} {}",
        weekday,
        target_date.format("%Y-%m-%d"),
        tz
    );
    println!("{}", "\u{2500}".repeat(45));

    if slots.is_empty() && all_day_events.is_empty() {
        println!("(no events)");
        return Ok(());
    }

    // Sort timed events by start time
    slots.sort_by_key(|s| s.start_min);

    // Get current time in minutes from midnight for the time marker (only for today)
    let now_min = if is_today {
        let now = Local::now().with_timezone(&tz);
        Some((now.hour() * 60 + now.minute()) as u32)
    } else {
        None
    };

    match view {
        AgendaView::List => render_list_view(&all_day_events, &slots),
        AgendaView::Gantt => render_gantt_view(&slots, now_min),
        AgendaView::Compact => render_compact_view(&all_day_events, &slots),
    }

    Ok(())
}

/// Clean subject by removing status prefixes.
fn clean_subject(subject: &str) -> String {
    let prefixes = [
        "Cancelled: ",
        "Abgesagt: ",
        "Blocker: ",
        "Blocked: ",
        "cancelled: ",
        "abgesagt: ",
        "blocker: ",
        "blocked: ",
    ];
    let mut s = subject.to_string();
    for prefix in &prefixes {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }
    s
}

/// Truncate string to max length, adding ellipsis if needed.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(1)).collect();
        format!("{}..", truncated)
    }
}

// Nerd Font icons
const ICON_CLOCK: &str = "\u{f017}"; // nf-fa-clock_o
const ICON_CALENDAR: &str = "\u{f073}"; // nf-fa-calendar
const ICON_LOCATION: &str = "\u{f041}"; // nf-fa-map_marker

/// Render the detailed list view (Option 1).
fn render_list_view(all_day: &[AgendaSlot], timed: &[AgendaSlot]) {
    // All-day events section
    if !all_day.is_empty() {
        println!();
        println!("{} ALL DAY", ICON_CALENDAR);
        for slot in all_day {
            let icon = slot.status.icon();
            println!("  \u{2022} {}{}", icon, slot.subject);
        }
        println!();
    }

    // Timed events
    for slot in timed {
        let time_range = format!("{}\u{2013}{}", slot.start_label, slot.end_label);
        let icon = slot.status.icon();
        println!("{:<14} {}{}", time_range, icon, slot.subject);

        if let Some(loc) = slot.location.as_ref().filter(|s| !s.is_empty()) {
            println!("{:14} {} {}", "", ICON_LOCATION, loc);
        }
    }
}

/// Render the Gantt-style timeline view (Option 2).
fn render_gantt_view(slots: &[AgendaSlot], now_min: Option<u32>) {
    if slots.is_empty() {
        return;
    }

    // Find the hour range to display
    let min_hour = slots.iter().map(|s| s.start_min / 60).min().unwrap_or(8);
    let max_hour = slots
        .iter()
        .map(|s| (s.end_min + 59) / 60)
        .max()
        .unwrap_or(18);

    // Clamp to reasonable range
    let start_hour = min_hour.min(8).max(0);
    let end_hour = max_hour.max(18).min(24);
    let hour_count = (end_hour - start_hour) as usize;

    // Calculate column width for labels (max short name + margin)
    let label_width = slots
        .iter()
        .map(|s| s.short_name.chars().count())
        .max()
        .unwrap_or(12)
        .max(12);

    // Characters per hour (4 chars = 15 min resolution for wider spacing)
    let chars_per_hour = 4usize;
    let bar_width = hour_count * chars_per_hour;

    // Calculate current time position (only if now_min is provided, i.e., today)
    let now_pos = now_min.and_then(|nm| {
        if nm >= start_hour * 60 && nm < end_hour * 60 {
            Some(((nm - start_hour * 60) as usize * chars_per_hour) / 60)
        } else {
            None
        }
    });

    // Print header with hour labels (spaced wider)
    print!("{} {:width$}", ICON_CLOCK, "Hours", width = label_width - 2);
    for h in start_hour..end_hour {
        print!("{:<4}", h);
    }
    println!();

    // Print separator using box drawing, with current time marker
    let mut sep: Vec<char> = "\u{2500}"
        .repeat(label_width + 1 + bar_width)
        .chars()
        .collect();
    if let Some(pos) = now_pos {
        let marker_pos = label_width + 1 + pos;
        if marker_pos < sep.len() {
            sep[marker_pos] = '\u{253c}'; // Box drawing cross
        }
    }
    println!("{}", sep.into_iter().collect::<String>());

    // Print each event as a row
    for slot in slots {
        if slot.all_day {
            continue; // Skip all-day events in gantt view
        }

        // Calculate bar positions
        let start_pos =
            ((slot.start_min.saturating_sub(start_hour * 60)) as usize * chars_per_hour) / 60;
        let end_pos =
            ((slot.end_min.saturating_sub(start_hour * 60)) as usize * chars_per_hour) / 60;

        let start_pos = start_pos.min(bar_width);
        let end_pos = end_pos.clamp(start_pos + 1, bar_width);

        // Build the bar using Unicode block characters
        let mut bar: Vec<char> = vec![' '; bar_width];
        let bar_char = slot.status.bar_char();
        for idx in start_pos..end_pos {
            bar[idx] = bar_char;
        }

        // Add current time marker if it falls within this row
        if let Some(pos) = now_pos {
            if pos < bar_width && bar[pos] == ' ' {
                bar[pos] = '\u{2502}'; // Vertical line
            }
        }

        let bar_str: String = bar.into_iter().collect();

        print!("{:<width$} ", slot.short_name, width = label_width);
        println!("{}", bar_str);
    }
}

/// Render the compact view (similar to ppl agenda style).
fn render_compact_view(all_day: &[AgendaSlot], timed: &[AgendaSlot]) {
    use owo_colors::OwoColorize;

    // All-day events
    for slot in all_day {
        let icon = slot.status.icon();
        println!(
            "  {} (all day)  {}{}",
            "\u{2588}",
            icon,
            slot.subject.dimmed()
        );
    }

    // Timed events
    for slot in timed {
        let time_range = format!("{}-{}", slot.start_label, slot.end_label);
        let icon = slot.status.icon();

        println!("  \u{2588} {:<13} {}{}", time_range, icon, slot.subject);

        if let Some(loc) = slot.location.as_ref().filter(|s| !s.is_empty()) {
            println!("    {} {}", ICON_LOCATION, loc.dimmed());
        }
    }
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
