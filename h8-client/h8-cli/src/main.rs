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
use serde_json::{Value, json};

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
        Command::Addr { command } => handle_addr(&ctx, command),
        Command::Free(cmd) => handle_free(&ctx, cmd),
        Command::Resource { command } => handle_resource(&ctx, command),
        Command::Ppl { command } => handle_ppl(&ctx, command),
        Command::Config { command } => handle_config(&ctx, command),
        Command::Init(cmd) => handle_init(&ctx, cmd),
        Command::Completions { shell } => handle_completions(shell),
        Command::Service { command } => handle_service(&ctx, command),
        Command::Which(args) => handle_natural_resource(&ctx, args),
        Command::Book(args) => handle_book(&ctx, args),
        Command::Trip(args) => handle_trip(&ctx, args),
        Command::Rules { command } => handle_rules(&ctx, command),
        Command::Oof { command } => handle_oof(&ctx, command),
        Command::Sync(args) => handle_sync(&ctx, args),
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
    /// Address operations (search cached, resolve from GAL)
    #[command(alias = "address")]
    Addr {
        #[command(subcommand)]
        command: AddrCommand,
    },
    Free(FreeCommand),
    /// Resource group operations (rooms, cars, equipment)
    #[command(alias = "res")]
    Resource {
        #[command(subcommand)]
        command: ResourceCommand,
    },
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
    /// Natural language resource query ("which cars are free tomorrow")
    ///
    /// Examples:
    ///   h8 which cars are free
    ///   h8 which cars are free on tuesday
    ///   h8 which rooms are free tomorrow afternoon
    ///   h8 which cars are free friday 13-15
    ///   h8 is the bmw free tomorrow
    ///   h8 cars free 20.03
    #[command(alias = "is")]
    Which(NaturalResourceArgs),
    /// Book a resource (room, car, etc.)
    ///
    /// Interactive: shows available resources and lets you pick one.
    /// Programmatic: use --json to get available resources, --select to book directly.
    ///
    /// Examples:
    ///   h8 book room today 12-14
    ///   h8 book car tomorrow 9-12
    ///   h8 book room friday 14-16 --select 02-41 --subject "Team Sync"
    ///   h8 book room today 12-14 --json
    Book(BookArgs),
    /// Plan a business trip: calculate travel, check resources, create calendar events
    ///
    /// Calculates travel time from your configured origin to the destination,
    /// optionally books a car, and creates calendar events for the full trip
    /// (travel to, meeting, travel back).
    ///
    /// Examples:
    ///   h8 trip Berlin friday 9-12 --car
    ///   h8 trip Munich tomorrow 14-16 --transit
    ///   h8 trip "Hamburg Hbf" monday 10-15 --car --book
    ///   h8 trip Berlin friday 9-12 --car --json
    ///   h8 trip Berlin friday 9-12 --car --from home
    Trip(TripArgs),
    /// Manage inbox rules (filters, auto-sorting, forwarding)
    ///
    /// Create rules to automatically organize your email:
    ///   h8 rules list
    ///   h8 rules create "move newsletters to Archive if subject contains 'Weekly'"
    ///   h8 rules create --name "Invoice Filter" --if from "billing@" --then move-to Archive
    ///   h8 rules enable <id>
    ///   h8 rules disable <id>
    ///   h8 rules delete <id>
    #[command(alias = "rule")]
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    /// Manage Out-of-Office (auto-reply) settings
    ///
    /// Enable, disable, or schedule automatic replies:
    ///   h8 oof status
    ///   h8 oof enable "I am out of office until Monday"
    ///   h8 oof schedule "2026-03-10" "2026-03-15" --message "On vacation"
    ///   h8 oof disable
    #[command(alias = "autoreply")]
    Oof {
        #[command(subcommand)]
        command: OofCommand,
    },
    /// Sync all data - calendar, emails, and contacts
    ///
    /// Fetches all data from the server and updates local cache.
    /// Performs incremental sync by default (only changes since last sync).
    ///
    /// Examples:
    ///   h8 sync                        # Sync everything (incremental)
    ///   h8 sync --full                 # Full re-sync of everything
    ///   h8 sync --calendar --mail      # Only sync calendar and mail
    ///   h8 sync -w 8 -p 2              # Sync 8 weeks future, 2 weeks past
    Sync(SyncArgs),
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
    ///
    /// Attendees can be added using "with <name>" - names are resolved from config aliases.
    /// Multiple attendees: "with alice, bob and charlie" or "with alice and bob"
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
    /// Bulk unsubscribe from marketing emails
    ///
    /// Scans messages for unsubscribe links (List-Unsubscribe header + body patterns),
    /// then visits them to unsubscribe. Dry run by default -- use --execute to act.
    ///
    /// Examples:
    ///   h8 mail unsubscribe --from newsletter@example.com
    ///   h8 mail unsubscribe --search "Weekly Digest" --dry-run
    ///   h8 mail unsubscribe --from marketing@ --execute
    ///   h8 mail unsubscribe --all --limit 100 --json
    #[command(alias = "unsub")]
    Unsubscribe(MailUnsubscribeArgs),
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

#[derive(Debug, Args)]
struct MailUnsubscribeArgs {
    /// Target specific sender (substring match, e.g., "newsletter@" or "marketing")
    #[arg(long)]
    from: Option<String>,
    /// Search for emails matching subject term
    #[arg(long)]
    search: Option<String>,
    /// Scan all emails in inbox
    #[arg(long)]
    all: bool,
    /// Actually perform unsubscribes (default is dry run / scan only)
    #[arg(long)]
    execute: bool,
    /// Maximum emails to process
    #[arg(short = 'l', long, default_value_t = 50)]
    limit: usize,
    /// Folder to scan
    #[arg(short = 'f', long, default_value = "inbox")]
    folder: String,
    /// Save results to JSON file
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
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

#[derive(Debug, Subcommand)]
enum AddrCommand {
    /// Search cached email addresses (from sent/received mail)
    #[command(alias = "ls")]
    Search(AddrSearchArgs),
    /// Resolve a name or email against the Global Address List (GAL)
    ///
    /// Uses EWS ResolveNames to find mailboxes including resource rooms,
    /// equipment, and distribution lists that are not in your contacts.
    Resolve(AddrResolveArgs),
}

#[derive(Debug, Args)]
struct AddrSearchArgs {
    /// Search query (matches email or name)
    query: Option<String>,
    /// Maximum results
    #[arg(short = 'l', long, default_value_t = 20)]
    limit: usize,
    /// Show most frequently used (ignore query)
    #[arg(long)]
    frequent: bool,
}

#[derive(Debug, Args)]
struct AddrResolveArgs {
    /// Search query (partial name, email prefix, etc.)
    query: String,
    /// Maximum results to display
    #[arg(short = 'l', long, default_value_t = 20)]
    limit: usize,
}

#[derive(Debug, Subcommand)]
enum ResourceCommand {
    /// Check resource availability (which are free?)
    ///
    /// Examples:
    ///   h8 resource free cars tomorrow
    ///   h8 resource free rooms friday 13-15
    ///   h8 resource free --all "next week"
    Free(ResourceFreeArgs),
    /// Show resource bookings/events
    ///
    /// Examples:
    ///   h8 resource agenda cars tomorrow
    ///   h8 resource agenda rooms friday
    Agenda(ResourceAgendaArgs),
    /// List configured resource groups
    #[command(alias = "ls")]
    List,
    /// Interactively add resources from the Global Address List
    ///
    /// Searches the GAL for resource mailboxes (rooms, cars, equipment),
    /// lets you pick which ones to add, and saves them to a resource group
    /// in config.toml.
    ///
    /// Examples:
    ///   h8 resource setup             # search and create a new group
    ///   h8 resource setup cars        # add resources to existing "cars" group
    ///   h8 resource setup --query "resource.m-em"  # pre-fill search query
    Setup(ResourceSetupArgs),
    /// Remove a resource from a group
    #[command(alias = "rm")]
    Remove(ResourceRemoveArgs),
}

#[derive(Debug, Args)]
struct ResourceFreeArgs {
    /// Resource group name (e.g., "cars", "rooms") or --all for all groups
    group: Option<String>,
    /// Time specification (natural language: tomorrow, friday, "next week", 20.03)
    #[arg(trailing_var_arg = true)]
    when: Vec<String>,
    /// Query all resource groups
    #[arg(long)]
    all: bool,
}

#[derive(Debug, Args)]
struct ResourceAgendaArgs {
    /// Resource group name (e.g., "cars", "rooms")
    group: String,
    /// Time specification (natural language)
    #[arg(trailing_var_arg = true)]
    when: Vec<String>,
}

#[derive(Debug, Args)]
struct ResourceSetupArgs {
    /// Target group name (e.g., "cars", "rooms"). Prompts if not given.
    group: Option<String>,
    /// Pre-fill the GAL search query
    #[arg(short = 'q', long)]
    query: Option<String>,
}

#[derive(Debug, Args)]
struct ResourceRemoveArgs {
    /// Resource group name
    group: String,
    /// Resource alias to remove
    alias: String,
}

#[derive(Debug, Args)]
struct NaturalResourceArgs {
    /// Natural language query (e.g., "cars are free tomorrow", "bmw free friday")
    #[arg(trailing_var_arg = true, required = true)]
    query: Vec<String>,
}

// === Rules Commands ===

#[derive(Debug, Subcommand)]
enum RulesCommand {
    /// List inbox rules
    #[command(alias = "ls")]
    List(RulesListArgs),
    /// Show rule details
    #[command(alias = "get")]
    Show(RulesShowArgs),
    /// Create a new rule
    ///
    /// Natural language (human-friendly):
    ///   h8 rules create "move newsletters to Archive if subject contains 'Weekly'"
    ///   h8 rules create "delete emails from spam@example.com"
    ///
    /// Structured (agent-friendly):
    ///   h8 rules create --name "Newsletter Filter" --if subject-contains "Weekly" --then move-to Archive
    Create(RulesCreateArgs),
    /// Enable a rule
    Enable(RulesEnableArgs),
    /// Disable a rule
    Disable(RulesDisableArgs),
    /// Delete a rule
    #[command(alias = "rm")]
    Delete(RulesDeleteArgs),
}

#[derive(Debug, Args)]
struct RulesListArgs {
    /// Show all details (default shows summary)
    #[arg(long)]
    detailed: bool,
}

#[derive(Debug, Args)]
struct RulesShowArgs {
    /// Rule ID (e.g., 'swift-owl')
    id: String,
}

#[derive(Debug, Args)]
struct RulesCreateArgs {
    /// Rule name/description (or full natural language rule)
    #[arg(required = true, num_args = 1..)]
    name: Vec<String>,
    /// Condition: from email address
    #[arg(long)]
    from: Option<String>,
    /// Condition: subject contains
    #[arg(long)]
    subject_contains: Option<String>,
    /// Condition: body contains
    #[arg(long)]
    body_contains: Option<String>,
    /// Condition: has attachments
    #[arg(long)]
    has_attachments: bool,
    /// Action: move to folder
    #[arg(long)]
    move_to: Option<String>,
    /// Action: copy to folder
    #[arg(long)]
    copy_to: Option<String>,
    /// Action: delete message
    #[arg(long)]
    delete: bool,
    /// Action: mark as read
    #[arg(long)]
    mark_read: bool,
    /// Action: forward to email(s)
    #[arg(long)]
    forward_to: Option<String>,
    /// Priority (1 = highest, default: 1)
    #[arg(long, default_value_t = 1)]
    priority: i32,
    /// Enable immediately (default: true)
    #[arg(long, default_value_t = true)]
    enabled: bool,
}

#[derive(Debug, Args)]
struct RulesEnableArgs {
    /// Rule ID
    id: String,
}

#[derive(Debug, Args)]
struct RulesDisableArgs {
    /// Rule ID
    id: String,
}

#[derive(Debug, Args)]
struct RulesDeleteArgs {
    /// Rule ID
    id: String,
    /// Skip confirmation
    #[arg(short = 'y', long)]
    yes: bool,
}

// === OOF Commands ===

#[derive(Debug, Subcommand)]
enum OofCommand {
    /// Show current OOF status
    Status,
    /// Enable OOF (immediate, not scheduled)
    ///
    /// Examples:
    ///   h8 oof enable "I am out of office until Monday"
    ///   h8 oof enable "On vacation" --external "Contact support for urgent matters"
    ///   h8 oof enable "Away" --audience known
    Enable(OofEnableArgs),
    /// Schedule OOF for a future period
    ///
    /// Examples:
    ///   h8 oof schedule "2026-03-10" "2026-03-15" --message "On vacation"
    ///   h8 oof schedule monday friday --message "In training"
    Schedule(OofScheduleArgs),
    /// Disable OOF
    Disable,
}

#[derive(Debug, Args)]
struct OofEnableArgs {
    /// Internal reply message
    #[arg(required = true, num_args = 1..)]
    message: Vec<String>,
    /// External reply message (defaults to internal)
    #[arg(long)]
    external: Option<String>,
    /// External audience: all, known, or none
    #[arg(long, default_value = "all")]
    audience: String,
}

#[derive(Debug, Args)]
struct OofScheduleArgs {
    /// Start date/time (ISO format or natural language)
    start: String,
    /// End date/time (ISO format or natural language)
    end: String,
    /// Internal reply message
    #[arg(short = 'm', long, required = true)]
    message: String,
    /// External reply message (defaults to internal)
    #[arg(long)]
    external: Option<String>,
    /// External audience: all, known, or none
    #[arg(long, default_value = "all")]
    audience: String,
}

#[derive(Debug, Args)]
struct BookArgs {
    /// Resource group name or individual alias (e.g., "room", "car", "bmw")
    resource: String,
    /// Time specification: date and time range
    /// Examples: "today 12-14", "friday 9-12", "tomorrow 14-16"
    #[arg(trailing_var_arg = true)]
    when: Vec<String>,
    /// Directly select a resource alias (skip interactive selection)
    #[arg(long)]
    select: Option<String>,
    /// Meeting subject (required for booking, prompted interactively if not given)
    #[arg(short = 's', long)]
    subject: Option<String>,
    /// Duration in minutes (defaults to full window)
    #[arg(short = 'd', long)]
    duration: Option<u32>,
}

#[derive(Debug, Args)]
struct TripArgs {
    /// Destination (city, address, or configured location alias)
    destination: String,
    /// Date and time range of the business at the destination
    /// Examples: "friday 9-12", "tomorrow 14-16", "20.03 10-15"
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    when: Vec<String>,
}

#[derive(Debug, Args)]
struct SyncArgs {
    /// Sync calendar events
    #[arg(long)]
    calendar: bool,
    /// Sync mail messages
    #[arg(long)]
    mail: bool,
    /// Sync contacts
    #[arg(long)]
    contacts: bool,
    /// Full re-sync (ignore sync tokens)
    #[arg(long)]
    full: bool,
    /// Weeks to sync into the future (default: 4)
    #[arg(short = 'w', long, default_value_t = 4)]
    weeks: i64,
    /// Weeks to sync into the past (default: 1)
    #[arg(short = 'p', long, default_value_t = 1)]
    past_weeks: i64,
    /// Only sync emails from last N days (for mail sync)
    #[arg(short = 'l', long)]
    limit_days: Option<u32>,
}

impl SyncArgs {
    /// Returns true if no specific data type was selected (sync everything)
    fn sync_everything(&self) -> bool {
        !self.calendar && !self.mail && !self.contacts
    }
}

/// Flags extracted from TripArgs trailing var arg.
#[derive(Debug, Default)]
struct TripFlags {
    car: bool,
    transit: bool,
    from: Option<String>,
    book: bool,
    select: Option<String>,
    subject: Option<String>,
    create: bool,
    sap: bool,
}

/// Parse trip-specific flags out of the trailing var arg words.
///
/// Returns (cleaned when words, extracted flags).
fn parse_trip_flags(words: &[String]) -> (Vec<String>, TripFlags) {
    let mut flags = TripFlags::default();
    let mut cleaned = Vec::new();
    let mut iter = words.iter().peekable();

    while let Some(word) = iter.next() {
        match word.as_str() {
            "--car" => flags.car = true,
            "--transit" | "--train" | "--public" => flags.transit = true,
            "--book" => flags.book = true,
            "--create" => flags.create = true,
            "--sap" => flags.sap = true,
            "--from" | "-f" => {
                if let Some(val) = iter.next() {
                    flags.from = Some(val.clone());
                }
            }
            "--select" => {
                if let Some(val) = iter.next() {
                    flags.select = Some(val.clone());
                }
            }
            "--subject" | "-s" => {
                if let Some(val) = iter.next() {
                    flags.subject = Some(val.clone());
                }
            }
            _ => cleaned.push(word.clone()),
        }
    }

    (cleaned, flags)
}

#[derive(Debug, Subcommand)]
enum PplCommand {
    /// View another person's calendar events
    Agenda(PplAgendaArgs),
    /// Find free slots in another person's calendar
    Free(PplFreeArgs),
    /// Find common free slots between multiple people
    Common(PplCommonArgs),
    /// Find common free slots and schedule a meeting (agent-friendly)
    ///
    /// Two-step workflow for automation:
    ///   1. List slots:  h8 ppl schedule alice bob -w 2 --json
    ///   2. Book a slot: h8 ppl schedule alice bob -w 2 --slot 1 -s "Review" --meeting-duration 45
    Schedule(PplScheduleArgs),
    /// Manage person aliases in config
    Alias {
        #[command(subcommand)]
        command: AliasCommand,
    },
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
    /// Interactive mode: select a slot and create a meeting (only in terminal)
    #[arg(short = 'i', long)]
    interactive: bool,
}

#[derive(Debug, Args)]
struct PplScheduleArgs {
    /// Person aliases or email addresses (2 or more)
    #[arg(required = true, num_args = 2..)]
    people: Vec<String>,
    /// Minimum free slot duration in minutes
    #[arg(short = 'd', long, default_value_t = 30)]
    duration: u32,
    /// Weeks to look ahead
    #[arg(short = 'w', long, default_value_t = 2)]
    weeks: u8,
    /// Maximum number of slots to return
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    /// Select slot by number (1-indexed) and create meeting
    #[arg(long)]
    slot: Option<usize>,
    /// Meeting subject (required when --slot is used)
    #[arg(short = 's', long)]
    subject: Option<String>,
    /// Meeting duration in minutes (required when --slot is used, max = slot duration)
    #[arg(short = 'm', long = "meeting-duration")]
    meeting_duration: Option<i64>,
    /// Meeting location
    #[arg(long)]
    location: Option<String>,
    /// Meeting body/description
    #[arg(short = 'b', long)]
    body: Option<String>,
}

#[derive(Debug, Subcommand)]
enum AliasCommand {
    /// List all person aliases
    #[command(alias = "ls")]
    List,
    /// Add a person alias
    Add(AliasAddArgs),
    /// Remove a person alias
    #[command(alias = "rm")]
    Remove(AliasRemoveArgs),
    /// Search cached email addresses
    Search(AliasSearchArgs),
    /// Interactively pick from recent email addresses and create aliases
    Pick(AliasPickArgs),
}

#[derive(Debug, Args)]
struct AliasAddArgs {
    /// Alias name (e.g., alice)
    name: String,
    /// Email address (e.g., alice@example.com)
    email: String,
}

#[derive(Debug, Args)]
struct AliasRemoveArgs {
    /// Alias name to remove
    name: String,
}

#[derive(Debug, Args)]
struct AliasSearchArgs {
    /// Search query (matches name or email)
    query: String,
    /// Maximum results
    #[arg(short = 'n', long, default_value_t = 20)]
    limit: usize,
}

#[derive(Debug, Args)]
struct AliasPickArgs {
    /// Number of recent addresses to show
    #[arg(short = 'n', long, default_value_t = 20)]
    limit: usize,
    /// Show most frequently used instead of most recent
    #[arg(long)]
    frequent: bool,
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

/// Strip global flags (--json, --yaml, --quiet etc.) that got captured by
/// `trailing_var_arg` and apply them to the context.
///
/// clap's `trailing_var_arg` consumes everything after positional args, including
/// global flags like `--json`. This function filters them out of the word list
/// and returns a potentially-modified context with those flags enabled.
fn strip_global_flags(ctx: &RuntimeContext, words: &[String]) -> (RuntimeContext, Vec<String>) {
    let mut common = ctx.common.clone();
    let mut filtered = Vec::with_capacity(words.len());

    for word in words {
        match word.as_str() {
            "--json" => common.json = true,
            "--yaml" => common.yaml = true,
            "--quiet" | "-q" => common.quiet = true,
            "--verbose" | "-v" => common.verbose += 1,
            "--debug" => common.debug = true,
            _ => filtered.push(word.clone()),
        }
    }

    let new_ctx = RuntimeContext {
        common,
        paths: ctx.paths.clone(),
        config: ctx.config.clone(),
    };
    (new_ctx, filtered)
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
            // Try local cache first
            let db_path = ctx.paths.sync_db_path(&account);
            let events_with_ids = if db_path.exists() {
                let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
                let cached = db
                    .search_calendar_events(&args.query, args.limit as usize)
                    .map_err(|e| anyhow!("{e}"))?;

                if !cached.is_empty() {
                    let events_json: Vec<serde_json::Value> = cached
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
                    // Nothing in cache, fall back to server
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
                    sync_calendar_events(ctx, &account, &events)?
                }
            } else {
                // No cache, use server
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
                sync_calendar_events(ctx, &account, &events)?
            };

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
                // Display attendees if present
                if let Some(attendees) = payload.get("attendees").and_then(|v| v.as_array()) {
                    if !attendees.is_empty() {
                        let emails: Vec<&str> = attendees
                            .iter()
                            .filter_map(|v| v.as_str())
                            .collect();
                        if !emails.is_empty() {
                            println!("  With: {}", emails.join(", "));
                        }
                    }
                }
                // Also check required_attendees and optional_attendees
                if let Some(req) = payload.get("required_attendees").and_then(|v| v.as_array()) {
                    if !req.is_empty() {
                        let emails: Vec<&str> = req
                            .iter()
                            .filter_map(|v| v.as_str())
                            .collect();
                        if !emails.is_empty() {
                            println!("  Required: {}", emails.join(", "));
                        }
                    }
                }
                if let Some(opt) = payload.get("optional_attendees").and_then(|v| v.as_array()) {
                    if !opt.is_empty() {
                        let emails: Vec<&str> = opt
                            .iter()
                            .filter_map(|v| v.as_str())
                            .collect();
                        if !emails.is_empty() {
                            println!("  Optional: {}", emails.join(", "));
                        }
                    }
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

    // 3a. Weekday + "next week" pattern (e.g., "friday next week", "next week friday")
    let next_week_re = Regex::new(r"(?i)\b(next\s+week|nächste\s+woche|naechste\s+woche)\b").unwrap();
    if next_week_re.is_match(&text_lower) {
        for (name, weekday) in weekdays {
            let pattern = format!(r"(?i)\b{}\b", regex::escape(name));
            if Regex::new(&pattern).unwrap().is_match(&text_lower) {
                // Calculate next week's occurrence of this weekday
                // First find next Monday
                let days_until_monday = (7 - now.weekday().num_days_from_monday()) % 7;
                let days_until_monday = if days_until_monday == 0 { 7 } else { days_until_monday };
                let next_monday = today + ChronoDuration::days(days_until_monday as i64);
                // Then find the weekday within next week (0 = Monday, 6 = Sunday)
                let target_offset = weekday.num_days_from_monday() as i64;
                let target = next_monday + ChronoDuration::days(target_offset);
                return Some((target, format!("{} next week", *name)));
            }
        }
    }

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
            // No year: use current year
            now.year()
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
                    // Use current year
                    now.year()
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
        MailCommand::Unsubscribe(args) => handle_mail_unsubscribe(ctx, &client, &account, args),
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

    // Resolve remote IDs to readable short IDs
    let db_path = ctx.paths.sync_db_path(account);
    if db_path.exists() {
        let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
        let id_gen = IdGenerator::new(&db);

        if let Some(msgs) = messages.as_array() {
            let resolved: Vec<Value> = msgs.iter().map(|msg| {
                let mut m = msg.clone();
                if let Some(remote_id) = msg.get("id").and_then(|v| v.as_str()) {
                    // Try existing short ID from id_pool, or from messages table,
                    // or allocate a new one
                    let short_id = db
                        .get_id_by_remote(remote_id)
                        .ok()
                        .flatten()
                        .or_else(|| {
                            db.get_message_by_remote_id(remote_id)
                                .ok()
                                .flatten()
                                .map(|m| m.local_id)
                        })
                        .or_else(|| id_gen.allocate(remote_id).ok());
                    if let Some(sid) = short_id {
                        m.as_object_mut().unwrap().insert("id".to_string(), json!(sid));
                    }
                }
                m
            }).collect();
            emit_output(&ctx.common, &json!(resolved))?;
            return Ok(());
        }
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
    }

    // Determine folders to sync
    let folders: Vec<String> = if let Some(folder) = args.folder {
        vec![folder]
    } else {
        ctx.config.mail.sync_folders.clone()
    };

    for folder in &folders {
        // Fetch metadata from server (fast - uses .only() fields, no bodies)
        let messages = client
            .mail_list(account, folder, 100, false)
            .map_err(|e| anyhow!("{e}"))?;

        let messages_arr = messages
            .as_array()
            .ok_or_else(|| anyhow!("expected array from server"))?;

        let mut synced = 0;
        let mut skipped = 0;

        for msg_val in messages_arr {
            // Apply cutoff filter
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
                .unwrap_or("");
            if remote_id.is_empty() {
                continue;
            }

            // Skip if already synced
            if db
                .get_message_by_remote_id(remote_id)
                .map_err(|e| anyhow!("{e}"))?
                .is_some()
            {
                skipped += 1;
                continue;
            }

            // Allocate human-readable ID
            let local_id = id_gen.allocate(remote_id).map_err(|e| anyhow!("{e}"))?;

            let subject = msg_val
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("(no subject)");
            let from = msg_val
                .get("from")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let date = msg_val
                .get("datetime_received")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_read = msg_val
                .get("is_read")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let has_attachments = msg_val
                .get("has_attachments")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Store metadata in database (body fetched on-demand via `h8 mail read`)
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
            if let Some((email, name)) = parse_email_address(from) {
                if folder == "sent" {
                    let _ = db.record_sent_address(&email, name.as_deref());
                } else {
                    let _ = db.record_received_address(&email, name.as_deref());
                }
            }

            synced += 1;
        }

        if !ctx.common.quiet {
            if synced > 0 {
                println!("  ✓ {}: {} new, {} up-to-date", folder, synced, skipped);
            } else {
                println!("  ✓ {}: {} up-to-date", folder, skipped);
            }
        }
    }

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

fn handle_mail_unsubscribe(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: MailUnsubscribeArgs,
) -> Result<()> {
    // Validate: must have --from, --search, or --all
    if args.from.is_none() && args.search.is_none() && !args.all {
        return Err(anyhow!(
            "specify --from <sender>, --search <term>, or --all to select messages"
        ));
    }

    let unsub_config = &ctx.config.unsubscribe;
    let limit = std::cmp::min(args.limit, unsub_config.max_emails_per_run);

    // Step 1: Scan for unsubscribe links
    if !ctx.common.quiet {
        if args.execute {
            println!("Scanning and unsubscribing from emails...\n");
        } else {
            println!("Scanning for unsubscribe links (dry run)...\n");
        }
    }

    let scan_results = client
        .mail_unsubscribe_scan(
            account,
            &args.folder,
            args.from.as_deref(),
            args.search.as_deref(),
            limit,
            &unsub_config.safe_senders,
            &unsub_config.blocked_patterns,
        )
        .map_err(|e| anyhow!("{e}"))?;

    let results_array = scan_results
        .as_array()
        .ok_or_else(|| anyhow!("expected array from scan"))?;

    if results_array.is_empty() {
        println!("No matching messages found.");
        return Ok(());
    }

    // Collect messages that have unsubscribe links
    let with_links: Vec<&Value> = results_array
        .iter()
        .filter(|r| {
            r.get("status").and_then(|s| s.as_str()) == Some("found")
        })
        .collect();

    let no_links: Vec<&Value> = results_array
        .iter()
        .filter(|r| {
            r.get("status").and_then(|s| s.as_str()) == Some("no_link")
        })
        .collect();

    let skipped: Vec<&Value> = results_array
        .iter()
        .filter(|r| {
            r.get("status").and_then(|s| s.as_str()) == Some("skipped")
        })
        .collect();

    if !ctx.common.json && !ctx.common.yaml {
        println!(
            "Scanned {} message(s): {} with unsubscribe links, {} without, {} skipped (safe sender)\n",
            results_array.len(),
            with_links.len(),
            no_links.len(),
            skipped.len(),
        );

        // Show scan results
        for result in results_array {
            let sender = result.get("sender").and_then(|v| v.as_str()).unwrap_or("?");
            let subject = result.get("subject").and_then(|v| v.as_str()).unwrap_or("(no subject)");
            let status = result.get("status").and_then(|v| v.as_str()).unwrap_or("?");
            let link_count = result
                .get("links")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);

            let status_indicator = match status {
                "found" => format!("[{} link(s)]", link_count),
                "no_link" => "[no link]".to_string(),
                "skipped" => "[safe sender]".to_string(),
                other => format!("[{}]", other),
            };

            println!("  {} {} - {}", status_indicator, sender, subject);
        }
        println!();
    }

    // Step 2: Execute if requested
    if args.execute && !with_links.is_empty() {
        let item_ids: Vec<String> = with_links
            .iter()
            .filter_map(|r| {
                r.get("message_id").and_then(|v| v.as_str()).map(String::from)
            })
            .collect();

        if !ctx.common.quiet {
            println!("Executing unsubscribe for {} message(s)...\n", item_ids.len());
        }

        let exec_results = client
            .mail_unsubscribe_execute(
                account,
                &item_ids,
                &unsub_config.safe_senders,
                &unsub_config.blocked_patterns,
                &unsub_config.trusted_unsubscribe_domains,
                unsub_config.rate_limit_seconds,
            )
            .map_err(|e| anyhow!("{e}"))?;

        if ctx.common.json || ctx.common.yaml {
            emit_output(&ctx.common, &exec_results)?;
        } else {
            let empty_vec = Vec::new();
            let exec_array = exec_results.as_array().unwrap_or(&empty_vec);
            let mut success = 0;
            let mut failed = 0;
            let mut needs_confirm = 0;

            for result in exec_array.iter() {
                let sender = result.get("sender").and_then(|v| v.as_str()).unwrap_or("?");
                let subject = result
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no subject)");
                let status = result.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                let error = result.get("error").and_then(|v| v.as_str());

                match status {
                    "success" => {
                        success += 1;
                        println!("  [OK] {} - {}", sender, subject);
                    }
                    "needs_confirmation" => {
                        needs_confirm += 1;
                        let detail = error.unwrap_or("needs manual confirmation");
                        println!("  [CONFIRM] {} - {} ({})", sender, subject, detail);
                    }
                    "skipped" => {
                        println!("  [SKIP] {} - {}", sender, subject);
                    }
                    _ => {
                        failed += 1;
                        let detail = error.unwrap_or("unknown error");
                        println!("  [FAIL] {} - {} ({})", sender, subject, detail);
                    }
                }
            }

            println!(
                "\nResults: {} success, {} need confirmation, {} failed",
                success, needs_confirm, failed
            );
        }

        // Save to output file if requested
        if let Some(ref output_path) = args.output {
            let json = serde_json::to_string_pretty(&exec_results)?;
            fs::write(output_path, json)?;
            if !ctx.common.quiet {
                println!("Results saved to {}", output_path.display());
            }
        }
    } else if !args.execute {
        // Dry run output
        if ctx.common.json || ctx.common.yaml {
            emit_output(&ctx.common, &scan_results)?;
        } else if !with_links.is_empty() {
            println!(
                "Use --execute to unsubscribe from {} message(s).",
                with_links.len()
            );
        }

        // Save scan results to output file if requested
        if let Some(ref output_path) = args.output {
            let json = serde_json::to_string_pretty(&scan_results)?;
            fs::write(output_path, json)?;
            if !ctx.common.quiet {
                println!("Scan results saved to {}", output_path.display());
            }
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

fn handle_addr(ctx: &RuntimeContext, command: AddrCommand) -> Result<()> {
    match command {
        AddrCommand::Search(args) => handle_addr_search(ctx, args),
        AddrCommand::Resolve(args) => handle_addr_resolve(ctx, args),
    }
}

fn handle_addr_search(ctx: &RuntimeContext, args: AddrSearchArgs) -> Result<()> {
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

fn handle_addr_resolve(ctx: &RuntimeContext, args: AddrResolveArgs) -> Result<()> {
    let account = effective_account(ctx);
    let client = ctx.service_client()?;

    let result = client
        .addr_resolve(&account, &args.query)
        .map_err(|e| anyhow!("{e}"))?;

    let empty = vec![];
    let entries = result.as_array().unwrap_or(&empty);

    if entries.is_empty() {
        println!("No results found for \"{}\"", args.query);
        return Ok(());
    }

    // Respect limit
    let limited: Vec<&Value> = entries.iter().take(args.limit).collect();

    if ctx.common.json || ctx.common.yaml {
        let limited_val: Vec<Value> = limited.into_iter().cloned().collect();
        emit_output(&ctx.common, &serde_json::to_value(&limited_val)?)?;
    } else {
        for entry in &limited {
            let name = entry["name"].as_str().unwrap_or("");
            let email = entry["email"].as_str().unwrap_or("?");
            let mbox_type = entry["mailbox_type"].as_str().unwrap_or("");
            if name.is_empty() {
                println!("{:<45} [{}]", email, mbox_type);
            } else {
                println!("{} <{}>  [{}]", name, email, mbox_type);
            }
        }
    }

    Ok(())
}

// =============================================================================
// Resource Handlers
// =============================================================================

fn handle_resource(ctx: &RuntimeContext, command: ResourceCommand) -> Result<()> {
    match command {
        ResourceCommand::Free(args) => handle_resource_free(ctx, args),
        ResourceCommand::Agenda(args) => handle_resource_agenda(ctx, args),
        ResourceCommand::List => handle_resource_list(ctx),
        ResourceCommand::Setup(args) => handle_resource_setup(ctx, args),
        ResourceCommand::Remove(args) => handle_resource_remove(ctx, args),
    }
}

/// Build a JSON array of resource items from a config resource group.
fn resource_group_to_json(group: &h8_core::ResourceGroup) -> Vec<Value> {
    group
        .iter()
        .map(|(alias, entry)| {
            serde_json::json!({
                "alias": alias,
                "email": entry.email(),
                "desc": entry.desc(),
            })
        })
        .collect()
}

/// Parse a time range like "13-15", "13:00-15:00", "9-12" from within text.
/// Returns (remaining_text, start_hour, start_min, end_hour, end_min) or None.
fn parse_hour_range(text: &str) -> Option<(String, u32, u32, u32, u32)> {
    use regex::Regex;
    let re = Regex::new(r"(\d{1,2})(?::(\d{2}))?\s*[-–]\s*(\d{1,2})(?::(\d{2}))?").unwrap();
    if let Some(caps) = re.captures(text) {
        let start_h: u32 = caps.get(1)?.as_str().parse().ok()?;
        let start_m: u32 = caps.get(2).map_or(0, |m| m.as_str().parse().unwrap_or(0));
        let end_h: u32 = caps.get(3)?.as_str().parse().ok()?;
        let end_m: u32 = caps.get(4).map_or(0, |m| m.as_str().parse().unwrap_or(0));

        // Sanity check: valid hours
        if start_h < 24 && end_h < 24 && start_h < end_h {
            let remaining = text.replace(caps.get(0)?.as_str(), "").trim().to_string();
            return Some((remaining, start_h, start_m, end_h, end_m));
        }
    }
    None
}

/// Parse time-of-day keywords: "morning" -> 9-12, "afternoon" -> 12-17, "evening" -> 17-20.
fn parse_time_of_day(text: &str) -> Option<(String, u32, u32)> {
    let text_lower = text.to_lowercase();
    let patterns = [
        ("morning", "morgens", "vormittag", 9, 12),
        ("afternoon", "nachmittag", "nachmittags", 12, 17),
        ("evening", "abend", "abends", 17, 20),
    ];
    for (en, de1, de2, start, end) in patterns {
        for keyword in [en, de1, de2] {
            if text_lower.contains(keyword) {
                let remaining = regex::RegexBuilder::new(&format!(r"\b{}\b", regex::escape(keyword)))
                    .case_insensitive(true)
                    .build()
                    .unwrap()
                    .replace(&text, "")
                    .trim()
                    .to_string();
                return Some((remaining, start, end));
            }
        }
    }
    None
}

fn handle_resource_free(ctx: &RuntimeContext, args: ResourceFreeArgs) -> Result<()> {
    // Strip global flags that trailing_var_arg may have captured
    let (ctx, when_cleaned) = strip_global_flags(ctx, &args.when);
    let ctx = &ctx;
    let args = ResourceFreeArgs {
        group: args.group,
        when: when_cleaned,
        all: args.all,
    };

    let account = effective_account(ctx);
    let client = ctx.service_client()?;

    if ctx.config.resources.is_empty() {
        return Err(anyhow!(
            "No resource groups configured. Add [resources.<group>] sections to config.toml"
        ));
    }

    // Determine which groups to query
    let groups_to_query: Vec<(String, Vec<Value>)> = if args.all {
        ctx.config
            .resources
            .iter()
            .map(|(name, group)| (name.clone(), resource_group_to_json(group)))
            .collect()
    } else {
        let group_name = args.group.as_deref().ok_or_else(|| {
            anyhow!(
                "Specify a resource group or use --all. Available: {}",
                ctx.config.resource_group_names().join(", ")
            )
        })?;
        let (_canon_name, group) = ctx.config.resource_group(group_name).ok_or_else(|| {
            anyhow!(
                "Unknown resource group '{}'. Available: {}",
                group_name,
                ctx.config.resource_group_names().join(", ")
            )
        })?;
        vec![(group_name.to_string(), resource_group_to_json(group))]
    };

    // Parse the when text
    let when_text = if args.when.is_empty() {
        "today".to_string()
    } else {
        args.when.join(" ")
    };

    // Check for hour range in the when text (e.g., "friday 13-15")
    let hour_range = parse_hour_range(&when_text);
    let time_of_day = if hour_range.is_none() {
        parse_time_of_day(&when_text)
    } else {
        None
    };

    // Determine the date text after removing time range / time-of-day
    let date_text = if let Some((ref remaining, ..)) = hour_range {
        remaining.clone()
    } else if let Some((ref remaining, ..)) = time_of_day {
        remaining.clone()
    } else {
        when_text.clone()
    };

    let date_text = if date_text.trim().is_empty() {
        "today".to_string()
    } else {
        date_text
    };

    // Parse the date
    let tz = ctx
        .config
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::UTC);

    let (from_date_str, to_date_str, description) = parse_date_range_expr(&date_text);

    // If we have a specific time window (hour range or time-of-day), use free-window endpoint
    let is_window_query = hour_range.is_some() || time_of_day.is_some();

    if is_window_query {
        let (start_h, start_m, end_h, end_m) = if let Some((_, sh, sm, eh, em)) = hour_range {
            (sh, sm, eh, em)
        } else if let Some((_, sh, eh)) = time_of_day {
            (sh, 0, eh, 0)
        } else {
            unreachable!()
        };

        // Parse the target date
        let target_date = if let Some((date, _desc)) = parse_single_date(&date_text) {
            date
        } else {
            Local::now().with_timezone(&tz).date_naive()
        };

        let window_start = format!(
            "{}T{:02}:{:02}:00",
            target_date.format("%Y-%m-%d"),
            start_h,
            start_m
        );
        let window_end = format!(
            "{}T{:02}:{:02}:00",
            target_date.format("%Y-%m-%d"),
            end_h,
            end_m
        );

        for (group_name, resources) in &groups_to_query {
            let result = client
                .resource_free_window(&account, resources, &window_start, &window_end)
                .map_err(|e| anyhow!("{e}"))?;

            if ctx.common.json || ctx.common.yaml {
                let output = serde_json::json!({
                    "group": group_name,
                    "window_start": window_start,
                    "window_end": window_end,
                    "resources": result,
                });
                emit_output(&ctx.common, &output)?;
            } else {
                println!(
                    "\n{} available {} {:02}:{:02}-{:02}:{:02}:\n",
                    capitalize(group_name),
                    description,
                    start_h,
                    start_m,
                    end_h,
                    end_m,
                );
                if let Some(entries) = result.as_array() {
                    for entry in entries {
                        let alias = entry["alias"].as_str().unwrap_or("?");
                        let desc = entry["desc"].as_str();
                        let available = entry["available"].as_bool().unwrap_or(false);
                        let label = if let Some(d) = desc {
                            format!("{} ({})", alias, d)
                        } else {
                            alias.to_string()
                        };
                        if available {
                            println!("  {:<35} -- available", label);
                        } else {
                            println!("  {:<35} -- booked", label);
                        }
                    }
                }
            }
        }
    } else {
        // General free query (show free slots per resource)
        for (group_name, resources) in &groups_to_query {
            let result = client
                .resource_free(
                    &account,
                    resources,
                    Some(&from_date_str),
                    Some(&to_date_str),
                    1,
                    None,
                    None,
                )
                .map_err(|e| anyhow!("{e}"))?;

            if ctx.common.json || ctx.common.yaml {
                let output = serde_json::json!({
                    "group": group_name,
                    "from": from_date_str,
                    "to": to_date_str,
                    "resources": result,
                });
                emit_output(&ctx.common, &output)?;
            } else {
                println!(
                    "\n{} available {} ({}):\n",
                    capitalize(group_name),
                    description,
                    from_date_str,
                );
                if let Some(entries) = result.as_array() {
                    for entry in entries {
                        let alias = entry["alias"].as_str().unwrap_or("?");
                        let desc = entry["desc"].as_str();
                        let slots = entry["free_slots"].as_array();

                        let label = if let Some(d) = desc {
                            format!("{} ({})", alias, d)
                        } else {
                            alias.to_string()
                        };

                        print!("  {}", label);

                        if let Some(slots) = slots {
                            if slots.is_empty() {
                                println!("\n    -- no availability --");
                            } else {
                                println!();
                                let slot_strs: Vec<String> = slots
                                    .iter()
                                    .filter_map(|s| {
                                        let start = s["start"].as_str()?;
                                        let end = s["end"].as_str()?;
                                        // Extract HH:MM from ISO datetime
                                        let start_time = &start[11..16];
                                        let end_time = &end[11..16];
                                        Some(format!("{} - {}", start_time, end_time))
                                    })
                                    .collect();
                                println!("    {}", slot_strs.join(", "));
                            }
                        } else {
                            println!("\n    -- no availability --");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn handle_resource_agenda(ctx: &RuntimeContext, args: ResourceAgendaArgs) -> Result<()> {
    // Strip global flags that trailing_var_arg may have captured
    let (ctx, when_cleaned) = strip_global_flags(ctx, &args.when);
    let ctx = &ctx;
    let args = ResourceAgendaArgs {
        group: args.group,
        when: when_cleaned,
    };

    let account = effective_account(ctx);
    let client = ctx.service_client()?;

    let (_canon_name, group) = ctx.config.resource_group(&args.group).ok_or_else(|| {
        anyhow!(
            "Unknown resource group '{}'. Available: {}",
            args.group,
            ctx.config.resource_group_names().join(", ")
        )
    })?;

    let resources = resource_group_to_json(group);

    let when_text = if args.when.is_empty() {
        "today".to_string()
    } else {
        args.when.join(" ")
    };

    let (from_date_str, to_date_str, description) = parse_date_range_expr(&when_text);

    let result = client
        .resource_agenda(
            &account,
            &resources,
            Some(&from_date_str),
            Some(&to_date_str),
            1,
        )
        .map_err(|e| anyhow!("{e}"))?;

    if ctx.common.json || ctx.common.yaml {
        let output = serde_json::json!({
            "group": args.group,
            "from": from_date_str,
            "to": to_date_str,
            "resources": result,
        });
        emit_output(&ctx.common, &output)?;
    } else {
        println!(
            "\n{} bookings {} ({}):\n",
            capitalize(&args.group),
            description,
            from_date_str,
        );
        if let Some(entries) = result.as_array() {
            for entry in entries {
                let alias = entry["alias"].as_str().unwrap_or("?");
                let desc = entry["desc"].as_str();
                let events = entry["events"].as_array();

                let label = if let Some(d) = desc {
                    format!("{} ({})", alias, d)
                } else {
                    alias.to_string()
                };

                print!("  {}", label);

                if let Some(events) = events {
                    if events.is_empty() {
                        println!("\n    -- no bookings --");
                    } else {
                        println!();
                        for ev in events {
                            let start = ev["start"].as_str().unwrap_or("?");
                            let end = ev["end"].as_str().unwrap_or("?");
                            let subject = ev["subject"].as_str().unwrap_or("");
                            let status = ev["status"].as_str().unwrap_or("");
                            let start_time = if start.len() >= 16 {
                                &start[11..16]
                            } else {
                                start
                            };
                            let end_time = if end.len() >= 16 { &end[11..16] } else { end };
                            if subject.is_empty() {
                                println!("    {} - {} [{}]", start_time, end_time, status);
                            } else {
                                println!(
                                    "    {} - {} {} [{}]",
                                    start_time, end_time, subject, status
                                );
                            }
                        }
                    }
                } else {
                    println!("\n    -- no data --");
                }
            }
        }
    }

    Ok(())
}

fn handle_resource_list(ctx: &RuntimeContext) -> Result<()> {
    if ctx.config.resources.is_empty() {
        println!("No resource groups configured.");
        println!("Add [resources.<group>] sections to config.toml");
        return Ok(());
    }

    if ctx.common.json || ctx.common.yaml {
        let output: Value = ctx
            .config
            .resources
            .iter()
            .map(|(group_name, group)| {
                let items: Vec<Value> = group
                    .iter()
                    .map(|(alias, entry)| {
                        serde_json::json!({
                            "alias": alias,
                            "email": entry.email(),
                            "desc": entry.desc(),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "group": group_name,
                    "resources": items,
                })
            })
            .collect();
        emit_output(&ctx.common, &output)?;
    } else {
        for (group_name, group) in &ctx.config.resources {
            println!("{}:", group_name);
            for (alias, entry) in group {
                if let Some(desc) = entry.desc() {
                    println!("  {:<15} {} ({})", alias, entry.email(), desc);
                } else {
                    println!("  {:<15} {}", alias, entry.email());
                }
            }
            println!();
        }
    }

    Ok(())
}

/// Parse natural language resource query.
///
/// Extracts resource group/alias and time spec from freeform text like:
/// - "which cars are free tomorrow"
/// - "bmw free friday 13-15"
/// - "any room available next week"
/// - "cars tuesday"
///
/// Returns (group_or_alias, time_words) where group_or_alias identifies
/// either a resource group name or a specific resource alias.
fn parse_natural_resource_query(
    text: &str,
    config: &AppConfig,
) -> Option<(NaturalResourceTarget, Vec<String>)> {
    // Normalize and tokenize
    let text_lower = text.to_lowercase();

    // Remove filler words
    let filler_re = regex::RegexBuilder::new(
        r"\b(which|are|is|the|a|an|any|free|frei|available|verfügbar|on|am|um|at)\b",
    )
    .case_insensitive(true)
    .build()
    .unwrap();
    let cleaned = filler_re.replace_all(&text_lower, " ");
    let cleaned = regex::Regex::new(r"\s+")
        .unwrap()
        .replace_all(&cleaned, " ");
    let cleaned = cleaned.trim().to_string();

    let words: Vec<&str> = cleaned.split_whitespace().collect();

    // Try to match a resource group name (could be multi-word but typically single)
    for (group_name, _group) in &config.resources {
        let gn_lower = group_name.to_lowercase();
        // Also try singular form (e.g., "car" -> "cars")
        let singular = gn_lower.trim_end_matches('s');

        for (idx, word) in words.iter().enumerate() {
            if *word == gn_lower || *word == singular {
                let time_words: Vec<String> = words
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != idx)
                    .map(|(_, w)| w.to_string())
                    .collect();
                return Some((
                    NaturalResourceTarget::Group(group_name.clone()),
                    time_words,
                ));
            }
        }
    }

    // Try to match a specific resource alias
    for (group_name, group) in &config.resources {
        for (alias, _entry) in group {
            let alias_lower = alias.to_lowercase();
            for (idx, word) in words.iter().enumerate() {
                if *word == alias_lower {
                    let time_words: Vec<String> = words
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != idx)
                        .map(|(_, w)| w.to_string())
                        .collect();
                    return Some((
                        NaturalResourceTarget::Single {
                            group: group_name.clone(),
                            alias: alias.clone(),
                        },
                        time_words,
                    ));
                }
            }
        }
    }

    None
}

#[derive(Debug)]
enum NaturalResourceTarget {
    Group(String),
    Single { group: String, alias: String },
}

fn handle_natural_resource(ctx: &RuntimeContext, args: NaturalResourceArgs) -> Result<()> {
    // Strip global flags that trailing_var_arg may have captured
    let (ctx, query_cleaned) = strip_global_flags(ctx, &args.query);
    let ctx = &ctx;
    let query_text = query_cleaned.join(" ");

    if ctx.config.resources.is_empty() {
        return Err(anyhow!(
            "No resource groups configured. Add [resources.<group>] sections to config.toml"
        ));
    }

    let (target, time_words) = parse_natural_resource_query(&query_text, &ctx.config)
        .ok_or_else(|| {
            anyhow!(
                "Could not identify a resource group or alias in: \"{}\"\nAvailable groups: {}",
                query_text,
                ctx.config.resource_group_names().join(", ")
            )
        })?;

    let when_text = if time_words.is_empty() {
        "today".to_string()
    } else {
        time_words.join(" ")
    };

    match target {
        NaturalResourceTarget::Group(group_name) => {
            // Delegate to resource free with the parsed group and time
            let group_arg = Some(group_name);
            let when_parts: Vec<String> = when_text.split_whitespace().map(|s| s.to_string()).collect();
            handle_resource_free(
                ctx,
                ResourceFreeArgs {
                    group: group_arg,
                    when: when_parts,
                    all: false,
                },
            )
        }
        NaturalResourceTarget::Single { group, alias } => {
            // Query just the single resource
            let account = effective_account(ctx);
            let client = ctx.service_client()?;

            let (email, desc) = ctx
                .config
                .resolve_resource(&group, &alias)
                .map_err(|e| anyhow!("{e}"))?;

            let resources = vec![serde_json::json!({
                "alias": alias,
                "email": email,
                "desc": desc,
            })];

            // Check for hour range
            let hour_range = parse_hour_range(&when_text);
            let time_of_day = if hour_range.is_none() {
                parse_time_of_day(&when_text)
            } else {
                None
            };

            let date_text = if let Some((ref remaining, ..)) = hour_range {
                if remaining.trim().is_empty() {
                    "today".to_string()
                } else {
                    remaining.clone()
                }
            } else if let Some((ref remaining, ..)) = time_of_day {
                if remaining.trim().is_empty() {
                    "today".to_string()
                } else {
                    remaining.clone()
                }
            } else {
                when_text.clone()
            };

            let tz = ctx
                .config
                .timezone
                .parse::<chrono_tz::Tz>()
                .unwrap_or(chrono_tz::UTC);

            let is_window = hour_range.is_some() || time_of_day.is_some();

            if is_window {
                let (start_h, start_m, end_h, end_m) =
                    if let Some((_, sh, sm, eh, em)) = hour_range {
                        (sh, sm, eh, em)
                    } else if let Some((_, sh, eh)) = time_of_day {
                        (sh, 0, eh, 0)
                    } else {
                        unreachable!()
                    };

                let target_date = if let Some((date, _)) = parse_single_date(&date_text) {
                    date
                } else {
                    Local::now().with_timezone(&tz).date_naive()
                };

                let window_start = format!(
                    "{}T{:02}:{:02}:00",
                    target_date.format("%Y-%m-%d"),
                    start_h,
                    start_m
                );
                let window_end = format!(
                    "{}T{:02}:{:02}:00",
                    target_date.format("%Y-%m-%d"),
                    end_h,
                    end_m
                );

                let result = client
                    .resource_free_window(&account, &resources, &window_start, &window_end)
                    .map_err(|e| anyhow!("{e}"))?;

                if ctx.common.json || ctx.common.yaml {
                    emit_output(&ctx.common, &result)?;
                } else if let Some(entries) = result.as_array() {
                    for entry in entries {
                        let available = entry["available"].as_bool().unwrap_or(false);
                        let label = if let Some(ref d) = desc {
                            format!("{} ({})", alias, d)
                        } else {
                            alias.clone()
                        };
                        if available {
                            println!(
                                "{} is free {} {:02}:{:02}-{:02}:{:02}",
                                label, date_text, start_h, start_m, end_h, end_m
                            );
                        } else {
                            println!(
                                "{} is booked {} {:02}:{:02}-{:02}:{:02}",
                                label, date_text, start_h, start_m, end_h, end_m
                            );
                        }
                    }
                }
            } else {
                let (from_date_str, to_date_str, description) =
                    parse_date_range_expr(&date_text);

                let result = client
                    .resource_free(
                        &account,
                        &resources,
                        Some(&from_date_str),
                        Some(&to_date_str),
                        1,
                        None,
                        None,
                    )
                    .map_err(|e| anyhow!("{e}"))?;

                if ctx.common.json || ctx.common.yaml {
                    emit_output(&ctx.common, &result)?;
                } else if let Some(entries) = result.as_array() {
                    for entry in entries {
                        let slots = entry["free_slots"].as_array();
                        let label = if let Some(ref d) = desc {
                            format!("{} ({})", alias, d)
                        } else {
                            alias.clone()
                        };

                        if let Some(slots) = slots {
                            if slots.is_empty() {
                                println!("{} has no availability {}", label, description);
                            } else {
                                println!(
                                    "{} is free {} ({}):",
                                    label, description, from_date_str
                                );
                                let slot_strs: Vec<String> = slots
                                    .iter()
                                    .filter_map(|s| {
                                        let start = s["start"].as_str()?;
                                        let end = s["end"].as_str()?;
                                        let start_time = &start[11..16];
                                        let end_time = &end[11..16];
                                        Some(format!("{} - {}", start_time, end_time))
                                    })
                                    .collect();
                                println!("  {}", slot_strs.join(", "));
                            }
                        }
                    }
                }
            }

            Ok(())
        }
    }
}

fn handle_resource_setup(ctx: &RuntimeContext, args: ResourceSetupArgs) -> Result<()> {
    use owo_colors::OwoColorize;

    if !io::stdout().is_terminal() {
        return Err(anyhow!("setup requires an interactive terminal"));
    }

    let account = effective_account(ctx);
    let client = ctx.service_client()?;

    // 1. Determine group name
    let group_name = if let Some(name) = args.group {
        name
    } else {
        let existing: Vec<String> = ctx.config.resource_group_names().iter().map(|s| s.to_string()).collect();
        if !existing.is_empty() {
            println!("Existing groups: {}", existing.join(", "));
        }
        dialoguer::Input::<String>::new()
            .with_prompt("Resource group name (e.g., cars, rooms)")
            .interact_text()
            .map_err(|e| anyhow!("input cancelled: {e}"))?
            .trim()
            .to_lowercase()
    };

    if group_name.is_empty() {
        return Err(anyhow!("group name cannot be empty"));
    }

    // 2. Search loop - allow multiple searches and accumulate selections
    let mut selected_resources: Vec<(String, String, Option<String>)> = Vec::new(); // (alias, email, desc)

    loop {
        let query = if let Some(ref q) = args.query {
            if selected_resources.is_empty() {
                q.clone()
            } else {
                dialoguer::Input::<String>::new()
                    .with_prompt("Search GAL (or press Enter to finish)")
                    .allow_empty(true)
                    .interact_text()
                    .map_err(|e| anyhow!("input cancelled: {e}"))?
            }
        } else {
            dialoguer::Input::<String>::new()
                .with_prompt(if selected_resources.is_empty() {
                    "Search GAL for resources"
                } else {
                    "Search again (or press Enter to finish)"
                })
                .allow_empty(!selected_resources.is_empty())
                .interact_text()
                .map_err(|e| anyhow!("input cancelled: {e}"))?
        };

        if query.trim().is_empty() {
            break;
        }

        println!("Searching...");
        let result = client
            .addr_resolve(&account, query.trim())
            .map_err(|e| anyhow!("{e}"))?;

        let entries = result.as_array().cloned().unwrap_or_default();
        if entries.is_empty() {
            println!("No results found for \"{}\". Try a different query.", query.trim());
            continue;
        }

        // Build display items, marking already-selected ones
        let already_selected_emails: std::collections::HashSet<String> = selected_resources
            .iter()
            .map(|(_, email, _)| email.to_lowercase())
            .collect();

        let display_entries: Vec<(usize, String)> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let email = e["email"].as_str().unwrap_or("").to_lowercase();
                !already_selected_emails.contains(&email)
            })
            .map(|(i, e)| {
                let name = e["name"].as_str().unwrap_or("");
                let email = e["email"].as_str().unwrap_or("?");
                let mbox_type = e["mailbox_type"].as_str().unwrap_or("");
                let label = if name.is_empty() {
                    format!("{:<50} [{}]", email, mbox_type)
                } else {
                    format!("{} <{}>  [{}]", name, email, mbox_type)
                };
                (i, label)
            })
            .collect();

        if display_entries.is_empty() {
            println!("All results already selected.");
            continue;
        }

        let items: Vec<&str> = display_entries.iter().map(|(_, s)| s.as_str()).collect();

        let selections = dialoguer::MultiSelect::new()
            .with_prompt("Select resources (Space to toggle, Enter to confirm)")
            .items(&items)
            .interact_opt()
            .map_err(|e| anyhow!("selection cancelled: {e}"))?;

        let selections = match selections {
            Some(s) if !s.is_empty() => s,
            _ => {
                println!("No resources selected from this search.");
                continue;
            }
        };

        // For each selected, prompt for alias and optional description
        for &sel_idx in &selections {
            let (orig_idx, _) = &display_entries[sel_idx];
            let entry = &entries[*orig_idx];
            let email = entry["email"].as_str().unwrap_or("").to_string();
            let gal_name = entry["name"].as_str().unwrap_or("").to_string();

            // Suggest alias from email local part
            let suggested_alias = email
                .split('@')
                .next()
                .unwrap_or("")
                .replace('.', "-")
                .to_lowercase();
            // Shorten if very long
            let suggested_alias = if suggested_alias.len() > 20 {
                suggested_alias[..20].to_string()
            } else {
                suggested_alias
            };

            let alias = dialoguer::Input::<String>::new()
                .with_prompt(format!("Alias for {}", email))
                .default(suggested_alias)
                .interact_text()
                .map_err(|e| anyhow!("input cancelled: {e}"))?
                .trim()
                .to_lowercase();

            if alias.is_empty() {
                println!("  Skipped {}", email);
                continue;
            }

            let desc_default = if gal_name.is_empty() {
                String::new()
            } else {
                gal_name.clone()
            };

            let desc = dialoguer::Input::<String>::new()
                .with_prompt(format!("Description for {} (optional)", alias))
                .default(desc_default)
                .allow_empty(true)
                .interact_text()
                .map_err(|e| anyhow!("input cancelled: {e}"))?
                .trim()
                .to_string();

            let desc_opt = if desc.is_empty() { None } else { Some(desc) };

            println!(
                "  {} {} -> {}{}",
                "+".green().bold(),
                alias.green(),
                email,
                desc_opt
                    .as_ref()
                    .map(|d| format!(" ({})", d))
                    .unwrap_or_default()
            );

            selected_resources.push((alias, email, desc_opt));
        }

        // If query was provided via --query, don't loop
        if args.query.is_some() && selected_resources.is_empty() {
            continue;
        }
        if args.query.is_some() {
            break;
        }
    }

    if selected_resources.is_empty() {
        println!("No resources added.");
        return Ok(());
    }

    // 3. Write to config.toml
    let (mut doc, config_path) = read_config_document(ctx)?;

    // Ensure [resources] table exists
    if doc.get("resources").is_none() {
        doc["resources"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let resources_table = doc["resources"]
        .as_table_mut()
        .ok_or_else(|| anyhow!("[resources] is not a table"))?;

    // Ensure [resources.<group>] table exists
    if resources_table.get(&group_name).is_none() {
        resources_table[&group_name] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let group_table = resources_table[&group_name]
        .as_table_mut()
        .ok_or_else(|| anyhow!("[resources.{}] is not a table", group_name))?;

    let mut added = 0;
    for (alias, email, desc) in &selected_resources {
        if group_table.contains_key(alias) {
            println!(
                "  Alias '{}' already exists in [resources.{}], skipping.",
                alias, group_name
            );
            continue;
        }

        if let Some(desc) = desc {
            // Use inline table: { email = "...", desc = "..." }
            let mut inline = toml_edit::InlineTable::new();
            inline.insert("email", email.as_str().into());
            inline.insert("desc", desc.as_str().into());
            group_table[alias] = toml_edit::value(inline);
        } else {
            // Simple string
            group_table[alias] = toml_edit::value(email.as_str());
        }
        added += 1;
    }

    if added > 0 {
        write_config_document(&doc, &config_path)?;
        println!(
            "\nSaved {} resource(s) to [resources.{}] in {}",
            added,
            group_name,
            config_path.display()
        );
    } else {
        println!("\nNo new resources added.");
    }

    Ok(())
}

fn handle_resource_remove(ctx: &RuntimeContext, args: ResourceRemoveArgs) -> Result<()> {
    use owo_colors::OwoColorize;

    let (mut doc, config_path) = read_config_document(ctx)?;

    let resources_table = doc
        .get_mut("resources")
        .and_then(|r| r.as_table_mut())
        .ok_or_else(|| anyhow!("no [resources] section in config"))?;

    let available_groups: Vec<String> = resources_table
        .iter()
        .map(|(k, _)| k.to_string())
        .collect();

    let group_table = resources_table
        .get_mut(&args.group)
        .and_then(|g| g.as_table_mut())
        .ok_or_else(|| {
            anyhow!(
                "unknown resource group '{}'. Available: {}",
                args.group,
                available_groups.join(", ")
            )
        })?;

    if !group_table.contains_key(&args.alias) {
        let available: Vec<&str> = group_table.iter().map(|(k, _)| k).collect();
        return Err(anyhow!(
            "unknown resource '{}' in group '{}'. Available: {}",
            args.alias,
            args.group,
            available.join(", ")
        ));
    }

    group_table.remove(&args.alias);
    write_config_document(&doc, &config_path)?;

    println!(
        "{} Removed '{}' from [resources.{}]",
        "-".red().bold(),
        args.alias,
        args.group
    );

    Ok(())
}

/// Format a NaiveDateTime as HH:MM.
fn trip_fmt_time(ndt: chrono::NaiveDateTime) -> String {
    ndt.format("%H:%M").to_string()
}

/// Parse ISO datetime string (RFC3339 or plain ISO) to NaiveDateTime.
fn trip_parse_iso_time(s: &str) -> Option<chrono::NaiveDateTime> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        Some(dt.naive_local())
    } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        Some(dt)
    } else {
        None
    }
}

/// Build a human-readable body string from transit journey legs for calendar events.
fn journey_body(journey: &serde_json::Value) -> Option<String> {
    let legs = journey["legs"].as_array()?;
    let mut lines = Vec::new();
    for leg in legs {
        let is_walking = leg["walking"].as_bool().unwrap_or(false);
        let dep_station = leg["departure_station"].as_str().unwrap_or("?");
        let arr_station = leg["arrival_station"].as_str().unwrap_or("?");
        let dep_time = leg["departure_time"].as_str()
            .and_then(|s| trip_parse_iso_time(s))
            .map(|dt| trip_fmt_time(dt))
            .unwrap_or_else(|| "??:??".to_string());
        let arr_time = leg["arrival_time"].as_str()
            .and_then(|s| trip_parse_iso_time(s))
            .map(|dt| trip_fmt_time(dt))
            .unwrap_or_else(|| "??:??".to_string());

        if is_walking {
            let distance = leg["distance_meters"].as_i64();
            let dur = leg["duration_minutes"].as_i64().unwrap_or(0);
            let dist_str = distance.map(|d| {
                if d >= 1000 { format!(" ({:.1} km)", d as f64 / 1000.0) }
                else { format!(" ({} m)", d) }
            }).unwrap_or_default();
            lines.push(format!(
                "{} Walk {}min {} -> {}{}",
                dep_time, dur, dep_station, arr_station, dist_str
            ));
        } else {
            let line_name = leg["line"].as_str().unwrap_or("?");
            let platform = leg["platform"].as_str();
            let arr_platform = leg["arrival_platform"].as_str();
            let dep_plat = platform.map(|p| format!(" (Gl. {})", p)).unwrap_or_default();
            let arr_plat = arr_platform.map(|p| format!(" (Gl. {})", p)).unwrap_or_default();
            lines.push(format!(
                "{} {} {}{} -> {} {}{}",
                dep_time, line_name, dep_station, dep_plat, arr_time, arr_station, arr_plat
            ));
        }
    }
    if lines.is_empty() { None } else { Some(lines.join("\n")) }
}

/// Extract non-walking line names from journey legs for the subject.
fn journey_line_names(journey: &serde_json::Value) -> Vec<String> {
    journey["legs"].as_array()
        .map(|legs| {
            legs.iter()
                .filter(|l| !l["walking"].as_bool().unwrap_or(false))
                .filter_map(|l| l["line"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn handle_trip(ctx: &RuntimeContext, args: TripArgs) -> Result<()> {
    use owo_colors::OwoColorize;

    // Strip global flags and trip-specific flags from trailing_var_arg
    let (ctx, global_cleaned) = strip_global_flags(ctx, &args.when);
    let ctx = &ctx;
    let (when_cleaned, flags) = parse_trip_flags(&global_cleaned);

    let client = ctx.service_client()?;
    let account = effective_account(ctx);
    let tz = ctx
        .config
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::UTC);

    // Determine transport mode
    let mode = if flags.transit {
        "transit"
    } else if flags.car {
        "car"
    } else {
        return Err(anyhow!("Specify transport mode: --car or --public (--transit/--train)"));
    };

    // Parse when: need date + time range
    let when_text = if when_cleaned.is_empty() {
        return Err(anyhow!(
            "Time range required. Example: h8 trip {} friday 9-12 --car",
            args.destination
        ));
    } else {
        when_cleaned.join(" ")
    };

    let hour_range = parse_hour_range(&when_text);
    let time_of_day = if hour_range.is_none() {
        parse_time_of_day(&when_text)
    } else {
        None
    };

    if hour_range.is_none() && time_of_day.is_none() {
        return Err(anyhow!(
            "Time range required. Examples:\n  h8 trip {} friday 9-12 --car\n  h8 trip {} tomorrow afternoon --transit",
            args.destination, args.destination
        ));
    }

    let (meeting_start_h, meeting_start_m, meeting_end_h, meeting_end_m) =
        if let Some((_, sh, sm, eh, em)) = &hour_range {
            (*sh, *sm, *eh, *em)
        } else if let Some((_, sh, eh)) = &time_of_day {
            (*sh, 0, *eh, 0)
        } else {
            unreachable!()
        };

    // Parse date
    let date_text = if let Some((ref remaining, ..)) = hour_range {
        if remaining.trim().is_empty() {
            "today".to_string()
        } else {
            remaining.clone()
        }
    } else if let Some((ref remaining, ..)) = time_of_day {
        if remaining.trim().is_empty() {
            "today".to_string()
        } else {
            remaining.clone()
        }
    } else {
        "today".to_string()
    };

    let target_date = if let Some((date, _)) = parse_single_date(&date_text) {
        date
    } else {
        Local::now().with_timezone(&tz).date_naive()
    };

    // Resolve origin location
    let origin_alias = flags
        .from
        .as_deref()
        .unwrap_or(&ctx.config.trip.default_origin);
    let origin = ctx.config.trip.resolve_location(origin_alias).ok_or_else(|| {
        let available: Vec<&str> = ctx.config.trip.locations.keys().map(|s| s.as_str()).collect();
        if available.is_empty() {
            anyhow!(
                "No origin location '{}' configured. Add [trip.locations.{}] to config.toml with address, lat, lon",
                origin_alias, origin_alias
            )
        } else {
            anyhow!(
                "Unknown origin '{}'. Available: {}",
                origin_alias,
                available.join(", ")
            )
        }
    })?;

    // Resolve destination: check configured locations first, then geocode
    let (dest_lat, dest_lon, dest_name) =
        if let Some(loc) = ctx.config.trip.resolve_location(&args.destination) {
            (loc.lat, loc.lon, loc.address.clone())
        } else {
            // Geocode the destination
            if !ctx.common.quiet {
                eprint!("Geocoding \"{}\"... ", args.destination);
            }
            let geo_result = client
                .trip_geocode(&args.destination, ctx.config.trip.country.as_deref())
                .map_err(|e| anyhow!("Geocoding failed: {e}"))?;
            if !ctx.common.quiet {
                eprintln!(
                    "{}",
                    geo_result["display_name"]
                        .as_str()
                        .unwrap_or("found")
                );
            }
            (
                geo_result["lat"].as_f64().ok_or_else(|| anyhow!("missing lat"))?,
                geo_result["lon"].as_f64().ok_or_else(|| anyhow!("missing lon"))?,
                geo_result["display_name"]
                    .as_str()
                    .unwrap_or(&args.destination)
                    .to_string(),
            )
        };

    // Calculate route
    if !ctx.common.quiet {
        eprint!("Calculating {} route... ", mode);
    }

    let origin_station = origin.station.as_deref();
    // For transit, try to derive station name from destination if not a configured location
    let dest_station_name: Option<String> = if mode == "transit" {
        if let Some(loc) = ctx.config.trip.resolve_location(&args.destination) {
            loc.station.clone()
        } else {
            Some(args.destination.clone())
        }
    } else {
        None
    };

    // For transit: query for "arrive by meeting start" (outbound)
    // For car: no time constraint on routing (just distance/duration)
    let arrival_param = if mode == "transit" {
        let meeting_start_dt = target_date
            .and_hms_opt(meeting_start_h, meeting_start_m, 0)
            .ok_or_else(|| anyhow!("invalid meeting start time"))?;
        Some(meeting_start_dt.format("%Y-%m-%dT%H:%M:%S").to_string())
    } else {
        None
    };

    let route_result = client
        .trip_route(
            origin.lat,
            origin.lon,
            dest_lat,
            dest_lon,
            mode,
            origin_station,
            dest_station_name.as_deref(),
            Some(&ctx.config.trip.transit_provider),
            None, // departure
            arrival_param.as_deref(), // arrival
        )
        .map_err(|e| {
            if mode == "transit" {
                anyhow!(
                    "Transit routing failed (the DB HAFAS API may be temporarily unavailable).\n\
                     Try again in a moment, or use --car instead.\n\
                     Details: {e}"
                )
            } else {
                anyhow!("Routing failed: {e}")
            }
        })?;

    let raw_travel_minutes = route_result["duration_minutes"]
        .as_i64()
        .ok_or_else(|| anyhow!("missing duration_minutes"))? as u32;
    let distance_km = route_result["distance_km"].as_f64();
    let buffer = ctx.config.trip.buffer_minutes;
    let round_step = ctx.config.trip.round_minutes;

    // Round travel time up to nearest step (for car; transit uses exact train times)
    let travel_minutes = if round_step > 0 && mode == "car" {
        ((raw_travel_minutes + round_step - 1) / round_step) * round_step
    } else {
        raw_travel_minutes
    };

    // Extract transit journey details if available
    let transit_journeys = route_result.get("transit_journeys")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if !ctx.common.quiet {
        eprintln!("done");
    }

    // Format helpers (thin wrappers around standalone functions for closure ergonomics)
    let fmt_time = |ndt: chrono::NaiveDateTime| -> String { trip_fmt_time(ndt) };
    let fmt_datetime = |ndt: chrono::NaiveDateTime| -> String {
        ndt.format("%Y-%m-%dT%H:%M:%S").to_string()
    };
    let parse_iso_time = |s: &str| -> Option<chrono::NaiveDateTime> { trip_parse_iso_time(s) };

    // Calculate full trip timeline
    let meeting_start = target_date
        .and_hms_opt(meeting_start_h, meeting_start_m, 0)
        .ok_or_else(|| anyhow!("invalid meeting start time"))?;
    let meeting_end = target_date
        .and_hms_opt(meeting_end_h, meeting_end_m, 0)
        .ok_or_else(|| anyhow!("invalid meeting end time"))?;
    let meeting_duration_min = (meeting_end - meeting_start).num_minutes();

    // For transit: use the best journey whose arrival is before meeting start
    // For car: use rounded travel time + buffer
    let (depart_at, arrive_back, outbound_journey, return_journey) = if mode == "transit" && !transit_journeys.is_empty() {
        // Outbound: first journey (arrives before meeting start thanks to arrival param)
        let outbound = transit_journeys.first().cloned();
        let ob_depart = outbound.as_ref()
            .and_then(|j| j["departure_time"].as_str())
            .and_then(|s| parse_iso_time(s))
            .unwrap_or_else(|| meeting_start - chrono::Duration::minutes((travel_minutes + buffer) as i64));
        let _ob_arrive = outbound.as_ref()
            .and_then(|j| j["arrival_time"].as_str())
            .and_then(|s| parse_iso_time(s))
            .unwrap_or(meeting_start);

        // Return journey: query departure from destination after meeting end
        eprint!("Calculating return route... ");
        let return_departure = meeting_end.format("%Y-%m-%dT%H:%M:%S").to_string();
        let return_result = client
            .trip_route(
                dest_lat,     // origin = destination (going back)
                dest_lon,
                origin.lat,   // dest = origin (going home)
                origin.lon,
                mode,
                dest_station_name.as_deref(),
                origin_station,
                Some(&ctx.config.trip.transit_provider),
                Some(&return_departure), // departure
                None,                     // no arrival constraint
            );
        let (ret_journey, ret_arrive) = match return_result {
            Ok(ref r) => {
                eprintln!("done");
                let ret_journeys: Vec<Value> = r["transit_journeys"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let first_ret = ret_journeys.into_iter().next();
                let arrive = first_ret.as_ref()
                    .and_then(|j| j["arrival_time"].as_str())
                    .and_then(|s| parse_iso_time(s))
                    .unwrap_or_else(|| meeting_end + chrono::Duration::minutes((travel_minutes + buffer) as i64));
                (first_ret, arrive)
            },
            Err(e) => {
                eprintln!("failed ({}), estimating return time", e);
                let arrive = meeting_end + chrono::Duration::minutes((travel_minutes + buffer) as i64);
                (None, arrive)
            },
        };

        (ob_depart, ret_arrive, outbound, ret_journey)
    } else {
        let dep = meeting_start - chrono::Duration::minutes((travel_minutes + buffer) as i64);
        let ret = meeting_end + chrono::Duration::minutes((travel_minutes + buffer) as i64);
        (dep, ret, None, None)
    };

    // Build trip data
    let mut trip_data = serde_json::json!({
        "destination": dest_name,
        "destination_lat": dest_lat,
        "destination_lon": dest_lon,
        "origin": origin.address,
        "origin_alias": origin_alias,
        "date": target_date.format("%Y-%m-%d").to_string(),
        "mode": mode,
        "travel_duration_minutes": travel_minutes,
        "travel_duration_raw_minutes": raw_travel_minutes,
        "round_minutes": round_step,
        "buffer_minutes": buffer,
        "distance_km": distance_km,
        "timeline": {
            "depart": fmt_datetime(depart_at),
            "meeting_start": fmt_datetime(meeting_start),
            "meeting_end": fmt_datetime(meeting_end),
            "arrive_back": fmt_datetime(arrive_back),
        },
        "meeting": {
            "start": fmt_datetime(meeting_start),
            "end": fmt_datetime(meeting_end),
            "duration_minutes": meeting_duration_min,
        },
        "route": route_result,
    });

    // Add transit journey details to top-level for easy access
    if let Some(ref outbound) = outbound_journey {
        trip_data["outbound_journey"] = outbound.clone();
    }
    if let Some(ref ret) = return_journey {
        trip_data["return_journey"] = ret.clone();
    }

    // JSON mode: output trip plan
    if ctx.common.json || ctx.common.yaml {
        emit_output(&ctx.common, &trip_data)?;
        return Ok(());
    }

    // Interactive display
    let date_display = target_date.format("%A, %Y-%m-%d").to_string();
    let travel_display = if mode == "car" {
        let rounded_note = if travel_minutes != raw_travel_minutes {
            format!(" (rounded from {}h{:02}m)", raw_travel_minutes / 60, raw_travel_minutes % 60)
        } else {
            String::new()
        };
        if let Some(km) = distance_km {
            format!("{}h{:02}m ({:.0} km){}", travel_minutes / 60, travel_minutes % 60, km, rounded_note)
        } else {
            format!("{}h{:02}m{}", travel_minutes / 60, travel_minutes % 60, rounded_note)
        }
    } else {
        format!("{}h{:02}m", travel_minutes / 60, travel_minutes % 60)
    };

    println!();
    println!("{}", format!("Trip to {}", dest_name).bold());
    println!("{}", "\u{2500}".repeat(60));
    println!("  Date:       {}", date_display);
    println!("  From:       {} ({})", origin.address, origin_alias);
    println!("  Transport:  {}", mode);
    println!("  Travel:     {}", travel_display);
    println!("  Buffer:     {} min", buffer);
    println!();
    println!("  {}", "Timeline:".bold());
    println!(
        "    {} Depart from {}",
        fmt_time(depart_at).yellow().bold(),
        origin_alias
    );

    // Show transit legs for outbound journey
    if let Some(ref journey) = outbound_journey {
        if let Some(legs) = journey["legs"].as_array() {
            let mut prev_arr_time: Option<chrono::NaiveDateTime> = None;
            for leg in legs {
                let is_walking = leg["walking"].as_bool().unwrap_or(false);
                let dep_station = leg["departure_station"].as_str().unwrap_or("?");
                let arr_station = leg["arrival_station"].as_str().unwrap_or("?");
                let dep_dt = leg["departure_time"].as_str().and_then(|s| parse_iso_time(s));
                let arr_dt = leg["arrival_time"].as_str().and_then(|s| parse_iso_time(s));
                let dep_time = dep_dt.map(|dt| fmt_time(dt)).unwrap_or_else(|| "??:??".to_string());
                let arr_time = arr_dt.map(|dt| fmt_time(dt)).unwrap_or_else(|| "??:??".to_string());

                // Show layover between legs
                if let (Some(prev), Some(dep)) = (prev_arr_time, dep_dt) {
                    let layover_min = (dep - prev).num_minutes();
                    if layover_min > 0 {
                        println!(
                            "      {} {} min layover",
                            "   ".dimmed(),
                            layover_min,
                        );
                    }
                }

                if is_walking {
                    let distance = leg["distance_meters"].as_i64();
                    let dur = leg["duration_minutes"].as_i64().unwrap_or(0);
                    let dist_str = distance.map(|d| {
                        if d >= 1000 { format!("{:.1} km", d as f64 / 1000.0) }
                        else { format!("{} m", d) }
                    }).unwrap_or_default();
                    println!(
                        "      {} {} walk {} -> {}{}",
                        dep_time.dimmed(),
                        format!("{}min", dur).dimmed(),
                        dep_station,
                        arr_station,
                        if !dist_str.is_empty() { format!(" ({})", dist_str).dimmed().to_string() } else { String::new() },
                    );
                } else {
                    let line = leg["line"].as_str().unwrap_or("?");
                    let platform = leg["platform"].as_str();
                    let arr_platform = leg["arrival_platform"].as_str();
                    let dep_plat = platform.map(|p| format!(" Gl. {}", p)).unwrap_or_default();
                    let arr_plat = arr_platform.map(|p| format!(" Gl. {}", p)).unwrap_or_default();
                    println!(
                        "      {} {} {}{} -> {} {}{}",
                        dep_time.cyan(),
                        line.bold(),
                        dep_station,
                        dep_plat.dimmed(),
                        arr_time.cyan(),
                        arr_station,
                        arr_plat.dimmed(),
                    );
                }
                prev_arr_time = arr_dt;
            }
            let changes = journey["changes"].as_i64().unwrap_or(0);
            if changes > 0 {
                println!("      ({} change{})", changes, if changes > 1 { "s" } else { "" });
            }
        }
    }

    println!(
        "    {} Arrive at {}",
        if let Some(ref j) = outbound_journey {
            j["arrival_time"].as_str()
                .and_then(|s| parse_iso_time(s))
                .map(|dt| fmt_time(dt))
                .unwrap_or_else(|| fmt_time(meeting_start))
        } else {
            fmt_time(meeting_start)
        }.dimmed(),
        args.destination
    );
    println!(
        "    {} Meeting ({} min)",
        format!("{}-{}", fmt_time(meeting_start), fmt_time(meeting_end))
            .green()
            .bold(),
        meeting_duration_min
    );
    println!(
        "    {} Depart from {}",
        if let Some(ref j) = return_journey {
            j["departure_time"].as_str()
                .and_then(|s| parse_iso_time(s))
                .map(|dt| fmt_time(dt))
                .unwrap_or_else(|| fmt_time(meeting_end))
        } else {
            fmt_time(meeting_end)
        }.dimmed(),
        args.destination
    );

    // Show transit legs for return journey
    if let Some(ref journey) = return_journey {
        if let Some(legs) = journey["legs"].as_array() {
            let mut prev_arr_time: Option<chrono::NaiveDateTime> = None;
            for leg in legs {
                let is_walking = leg["walking"].as_bool().unwrap_or(false);
                let dep_station = leg["departure_station"].as_str().unwrap_or("?");
                let arr_station = leg["arrival_station"].as_str().unwrap_or("?");
                let dep_dt = leg["departure_time"].as_str().and_then(|s| parse_iso_time(s));
                let arr_dt = leg["arrival_time"].as_str().and_then(|s| parse_iso_time(s));
                let dep_time = dep_dt.map(|dt| fmt_time(dt)).unwrap_or_else(|| "??:??".to_string());
                let arr_time = arr_dt.map(|dt| fmt_time(dt)).unwrap_or_else(|| "??:??".to_string());

                // Show layover between legs
                if let (Some(prev), Some(dep)) = (prev_arr_time, dep_dt) {
                    let layover_min = (dep - prev).num_minutes();
                    if layover_min > 0 {
                        println!(
                            "      {} {} min layover",
                            "   ".dimmed(),
                            layover_min,
                        );
                    }
                }

                if is_walking {
                    let distance = leg["distance_meters"].as_i64();
                    let dur = leg["duration_minutes"].as_i64().unwrap_or(0);
                    let dist_str = distance.map(|d| {
                        if d >= 1000 { format!("{:.1} km", d as f64 / 1000.0) }
                        else { format!("{} m", d) }
                    }).unwrap_or_default();
                    println!(
                        "      {} {} walk {} -> {}{}",
                        dep_time.dimmed(),
                        format!("{}min", dur).dimmed(),
                        dep_station,
                        arr_station,
                        if !dist_str.is_empty() { format!(" ({})", dist_str).dimmed().to_string() } else { String::new() },
                    );
                } else {
                    let line = leg["line"].as_str().unwrap_or("?");
                    let platform = leg["platform"].as_str();
                    let arr_platform = leg["arrival_platform"].as_str();
                    let dep_plat = platform.map(|p| format!(" Gl. {}", p)).unwrap_or_default();
                    let arr_plat = arr_platform.map(|p| format!(" Gl. {}", p)).unwrap_or_default();
                    println!(
                        "      {} {} {}{} -> {} {}{}",
                        dep_time.cyan(),
                        line.bold(),
                        dep_station,
                        dep_plat.dimmed(),
                        arr_time.cyan(),
                        arr_station,
                        arr_plat.dimmed(),
                    );
                }
                prev_arr_time = arr_dt;
            }
            let changes = journey["changes"].as_i64().unwrap_or(0);
            if changes > 0 {
                println!("      ({} change{})", changes, if changes > 1 { "s" } else { "" });
            }
        }
    }

    println!(
        "    {} Back at {}",
        fmt_time(arrive_back).yellow().bold(),
        origin_alias
    );

    // Car booking
    if mode == "car" && (flags.book || flags.select.is_some()) {
        println!();

        // Check car availability for the full trip window
        let car_group = ctx.config.resource_group("cars");
        if car_group.is_none() {
            println!(
                "{}",
                "No [resources.cars] group configured - skipping car booking".yellow()
            );
        } else {
            let (_, group) = car_group.unwrap();
            let resources = resource_group_to_json(group);

            let window_start = fmt_datetime(depart_at);
            let window_end = fmt_datetime(arrive_back);

            let avail = client
                .resource_free_window(&account, &resources, &window_start, &window_end)
                .map_err(|e| anyhow!("Car availability check failed: {e}"))?;

            let entries = avail
                .as_array()
                .ok_or_else(|| anyhow!("unexpected availability response"))?;

            let available: Vec<&Value> = entries
                .iter()
                .filter(|e| e["available"].as_bool() == Some(true))
                .collect();

            if available.is_empty() {
                println!("{}", "No cars available for the full trip window".red());
            } else if let Some(ref selected) = flags.select {
                // Direct car selection
                let subject = flags
                    .subject
                    .as_deref()
                    .unwrap_or("Business Trip");
                book_resource(
                    ctx,
                    &client,
                    &account,
                    entries,
                    selected,
                    subject,
                    &window_start,
                    &window_end,
                    None,
                )?;
            } else {
                // Interactive car selection
                println!("{}", "Available cars:".bold());
                let mut selectable: Vec<(usize, &Value)> = Vec::new();
                for (i, entry) in entries.iter().enumerate() {
                    let alias = entry["alias"].as_str().unwrap_or("?");
                    let desc = entry["desc"].as_str();
                    let is_avail = entry["available"].as_bool() == Some(true);
                    let label = if let Some(d) = desc {
                        format!("{} ({})", alias, d)
                    } else {
                        alias.to_string()
                    };

                    if is_avail {
                        selectable.push((i, entry));
                        println!(
                            "  [{}] {}  {}",
                            selectable.len().to_string().yellow().bold(),
                            label,
                            "available".green()
                        );
                    } else {
                        println!("      {}  {}", label, "booked".red().dimmed());
                    }
                }

                if !selectable.is_empty() && io::stdout().is_terminal() {
                    print!(
                        "\n{} ",
                        format!(
                            "Book a car? (1-{}, or 'n' to skip):",
                            selectable.len()
                        )
                        .cyan()
                    );
                    io::stdout().flush()?;

                    let mut input = String::new();
                    io::stdin().read_line(&mut input)?;
                    let input = input.trim();

                    if !input.eq_ignore_ascii_case("n")
                        && !input.eq_ignore_ascii_case("no")
                        && !input.is_empty()
                    {
                        if let Ok(n) = input.parse::<usize>() {
                            if n > 0 && n <= selectable.len() {
                                let (_, entry) = selectable[n - 1];
                                let alias =
                                    entry["alias"].as_str().unwrap_or("?");
                                let subject = if let Some(ref s) = flags.subject {
                                    s.clone()
                                } else {
                                    format!("Business Trip - {}", args.destination)
                                };
                                book_resource(
                                    ctx,
                                    &client,
                                    &account,
                                    entries,
                                    alias,
                                    &subject,
                                    &window_start,
                                    &window_end,
                                    None,
                                )?;
                            }
                        }
                    }
                }
            }
        }
    }

    // Calendar event creation
    if flags.create {
        println!();
        let subject = flags
            .subject
            .clone()
            .unwrap_or_else(|| format!("Business Trip - {}", args.destination));

        // Build travel-to body with transit details if available
        let travel_to_body = outbound_journey.as_ref().and_then(|j| journey_body(j));

        // Travel-to subject with transit line names
        let travel_to_subject = if let Some(ref journey) = outbound_journey {
            let names = journey_line_names(journey);
            if !names.is_empty() {
                format!("Travel to {} ({})", args.destination, names.join(" + "))
            } else {
                format!("Travel to {}", args.destination)
            }
        } else {
            format!("Travel to {}", args.destination)
        };

        let mut travel_to_payload = serde_json::json!({
            "subject": travel_to_subject,
            "start": fmt_datetime(depart_at),
            "end": fmt_datetime(meeting_start),
            "location": origin.address,
        });
        if let Some(body) = &travel_to_body {
            travel_to_payload["body"] = serde_json::json!(body);
        }
        client
            .calendar_create(&account, travel_to_payload)
            .map_err(|e| anyhow!("Failed to create travel-to event: {e}"))?;
        println!(
            "  {} Created: {} ({}-{})",
            "+".green().bold(),
            travel_to_subject,
            fmt_time(depart_at),
            fmt_time(meeting_start)
        );

        // Meeting
        let meeting_payload = serde_json::json!({
            "subject": subject,
            "start": fmt_datetime(meeting_start),
            "end": fmt_datetime(meeting_end),
            "location": dest_name,
        });
        client
            .calendar_create(&account, meeting_payload)
            .map_err(|e| anyhow!("Failed to create meeting event: {e}"))?;
        println!(
            "  {} Created: {} ({}-{})",
            "+".green().bold(),
            subject,
            fmt_time(meeting_start),
            fmt_time(meeting_end)
        );

        // Travel back
        let travel_back_body = return_journey.as_ref().and_then(|j| journey_body(j));
        let travel_back_subject = if let Some(ref journey) = return_journey {
            let names = journey_line_names(journey);
            if !names.is_empty() {
                format!("Travel from {} ({})", args.destination, names.join(" + "))
            } else {
                format!("Travel from {}", args.destination)
            }
        } else {
            format!("Travel from {}", args.destination)
        };

        let mut travel_back_payload = serde_json::json!({
            "subject": travel_back_subject,
            "start": fmt_datetime(meeting_end),
            "end": fmt_datetime(arrive_back),
            "location": dest_name,
        });
        if let Some(body) = &travel_back_body {
            travel_back_payload["body"] = serde_json::json!(body);
        }
        client
            .calendar_create(&account, travel_back_payload)
            .map_err(|e| anyhow!("Failed to create travel-back event: {e}"))?;
        println!(
            "  {} Created: {} ({}-{})",
            "+".green().bold(),
            travel_back_subject,
            fmt_time(meeting_end),
            fmt_time(arrive_back)
        );
    }

    // SAP export
    if flags.sap {
        let sap_data = serde_json::json!({
            "trip_type": "business_trip",
            "destination": dest_name,
            "origin": origin.address,
            "date": target_date.format("%Y-%m-%d").to_string(),
            "transport_mode": mode,
            "distance_km": distance_km,
            "travel_duration_minutes": travel_minutes,
            "depart_time": fmt_datetime(depart_at),
            "meeting_start": fmt_datetime(meeting_start),
            "meeting_end": fmt_datetime(meeting_end),
            "return_time": fmt_datetime(arrive_back),
            "total_duration_minutes": (arrive_back - depart_at).num_minutes(),
        });
        if ctx.common.json || ctx.common.yaml {
            emit_output(&ctx.common, &sap_data)?;
        } else {
            println!("\n{}", "SAP Export Data:".bold());
            println!("{}", serde_json::to_string_pretty(&sap_data)?);
        }
    }

    Ok(())
}

fn handle_book(ctx: &RuntimeContext, args: BookArgs) -> Result<()> {
    use owo_colors::OwoColorize;

    // Strip global flags from trailing_var_arg
    let (ctx, when_cleaned) = strip_global_flags(ctx, &args.when);
    let ctx = &ctx;

    let account = effective_account(ctx);
    let client = ctx.service_client()?;

    if ctx.config.resources.is_empty() {
        return Err(anyhow!(
            "No resource groups configured. Add [resources.<group>] sections to config.toml"
        ));
    }

    // Resolve "room" -> "rooms", "car" -> "cars" etc. (singular/plural)
    let resource_input = args.resource.to_lowercase();
    let (group_name, resources) = if let Some((_name, group)) = ctx.config.resource_group(&resource_input) {
        (resource_input.clone(), resource_group_to_json(group))
    } else {
        // Try singular -> plural
        let plural = format!("{}s", resource_input);
        if let Some((_name, group)) = ctx.config.resource_group(&plural) {
            (plural, resource_group_to_json(group))
        } else if let Some((grp, alias, email, desc)) = ctx.config.find_resource_by_alias(&resource_input) {
            // Single resource alias
            (grp, vec![serde_json::json!({ "alias": alias, "email": email, "desc": desc })])
        } else {
            return Err(anyhow!(
                "Unknown resource group or alias '{}'. Available groups: {}",
                resource_input,
                ctx.config.resource_group_names().join(", ")
            ));
        }
    };

    // Parse when text - must contain a time range
    let when_text = if when_cleaned.is_empty() {
        return Err(anyhow!("Time range required. Example: h8 book {} today 12-14", resource_input));
    } else {
        when_cleaned.join(" ")
    };

    let hour_range = parse_hour_range(&when_text);
    let time_of_day = if hour_range.is_none() {
        parse_time_of_day(&when_text)
    } else {
        None
    };

    if hour_range.is_none() && time_of_day.is_none() {
        return Err(anyhow!(
            "Time range required. Examples:\n  h8 book {} today 12-14\n  h8 book {} friday afternoon\n  h8 book {} tomorrow 9:00-11:30",
            resource_input, resource_input, resource_input
        ));
    }

    let (start_h, start_m, end_h, end_m) = if let Some((_, sh, sm, eh, em)) = &hour_range {
        (*sh, *sm, *eh, *em)
    } else if let Some((_, sh, eh)) = &time_of_day {
        (*sh, 0, *eh, 0)
    } else {
        unreachable!()
    };

    // Parse date
    let date_text = if let Some((ref remaining, ..)) = hour_range {
        if remaining.trim().is_empty() { "today".to_string() } else { remaining.clone() }
    } else if let Some((ref remaining, ..)) = time_of_day {
        if remaining.trim().is_empty() { "today".to_string() } else { remaining.clone() }
    } else {
        "today".to_string()
    };

    let tz = ctx.config.timezone.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC);
    let target_date = if let Some((date, _)) = parse_single_date(&date_text) {
        date
    } else {
        Local::now().with_timezone(&tz).date_naive()
    };

    let window_start = format!("{}T{:02}:{:02}:00", target_date.format("%Y-%m-%d"), start_h, start_m);
    let window_end = format!("{}T{:02}:{:02}:00", target_date.format("%Y-%m-%d"), end_h, end_m);

    // Query availability
    let result = client
        .resource_free_window(&account, &resources, &window_start, &window_end)
        .map_err(|e| anyhow!("{e}"))?;

    let entries = result.as_array().ok_or_else(|| anyhow!("unexpected response"))?;

    let available: Vec<&Value> = entries.iter().filter(|e| e["available"].as_bool() == Some(true)).collect();
    let date_display = target_date.format("%A %Y-%m-%d").to_string();

    // JSON mode: output availability and exit
    if ctx.common.json || ctx.common.yaml {
        let output = serde_json::json!({
            "group": group_name,
            "date": target_date.format("%Y-%m-%d").to_string(),
            "window_start": window_start,
            "window_end": window_end,
            "resources": entries,
            "available_count": available.len(),
        });
        emit_output(&ctx.common, &output)?;

        // If --select was given in JSON mode, also book and output the result
        if let Some(ref selected_alias) = args.select {
            let subject = args.subject.as_deref().ok_or_else(|| {
                anyhow!("--subject is required when using --select in JSON mode")
            })?;
            return book_resource(ctx, &client, &account, entries, selected_alias, subject, &window_start, &window_end, args.duration);
        }
        return Ok(());
    }

    // Non-JSON: interactive or direct booking
    if available.is_empty() {
        println!("\nNo {} available {} {:02}:{:02}-{:02}:{:02}",
            group_name, date_display, start_h, start_m, end_h, end_m);

        // Show which are booked
        for entry in entries {
            let alias = entry["alias"].as_str().unwrap_or("?");
            let desc = entry["desc"].as_str();
            let label = if let Some(d) = desc { format!("{} ({})", alias, d) } else { alias.to_string() };
            println!("  {:<35} -- booked", label);
        }
        return Ok(());
    }

    // Direct selection via --select
    if let Some(ref selected_alias) = args.select {
        let subject = args.subject.as_deref().ok_or_else(|| {
            anyhow!("--subject is required when using --select")
        })?;
        return book_resource(ctx, &client, &account, entries, selected_alias, subject, &window_start, &window_end, args.duration);
    }

    // Interactive selection
    if !io::stdout().is_terminal() {
        return Err(anyhow!(
            "Interactive mode requires a terminal. Use --json to get availability, or --select <alias> --subject <text> to book directly."
        ));
    }

    println!(
        "\n{} available {} {:02}:{:02}-{:02}:{:02}:\n",
        capitalize(&group_name),
        date_display,
        start_h, start_m, end_h, end_m,
    );

    // Show all resources with availability status
    let mut selectable: Vec<(usize, &Value)> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let alias = entry["alias"].as_str().unwrap_or("?");
        let desc = entry["desc"].as_str();
        let is_available = entry["available"].as_bool() == Some(true);
        let label = if let Some(d) = desc { format!("{} ({})", alias, d) } else { alias.to_string() };

        if is_available {
            selectable.push((i, entry));
            println!(
                "  [{}] {:<35} {}",
                selectable.len().to_string().yellow().bold(),
                label,
                "available".green()
            );
        } else {
            println!(
                "      {:<35} {}",
                label,
                "booked".red().dimmed()
            );
        }
    }

    if selectable.is_empty() {
        return Ok(());
    }

    // Prompt for selection
    print!(
        "\n{} ",
        format!("Select resource (1-{}), or 'c' to cancel:", selectable.len()).cyan()
    );
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.eq_ignore_ascii_case("c") || input.eq_ignore_ascii_case("cancel") || input.is_empty() {
        println!("Cancelled.");
        return Ok(());
    }

    let sel_idx: usize = match input.parse::<usize>() {
        Ok(n) if n > 0 && n <= selectable.len() => n - 1,
        _ => {
            println!("Invalid selection.");
            return Ok(());
        }
    };

    let (_orig_idx, selected_entry) = selectable[sel_idx];
    let alias = selected_entry["alias"].as_str().unwrap_or("?");
    let email = selected_entry["email"].as_str().unwrap_or("?");
    let desc = selected_entry["desc"].as_str();

    // Get subject
    let subject = if let Some(ref s) = args.subject {
        s.clone()
    } else {
        print!("{} ", "Meeting subject:".cyan());
        io::stdout().flush()?;
        let mut subj = String::new();
        io::stdin().read_line(&mut subj)?;
        let subj = subj.trim().to_string();
        if subj.is_empty() {
            println!("Cancelled - subject required.");
            return Ok(());
        }
        subj
    };

    // Calculate actual times
    let (book_start, book_end) = if let Some(dur) = args.duration {
        let end = format!(
            "{}T{:02}:{:02}:00",
            target_date.format("%Y-%m-%d"),
            start_h + dur / 60,
            start_m + dur % 60
        );
        (window_start.clone(), end)
    } else {
        (window_start.clone(), window_end.clone())
    };

    // Confirm
    let label = if let Some(d) = desc { format!("{} ({})", alias, d) } else { alias.to_string() };
    let start_time = format!("{:02}:{:02}", start_h, start_m);
    let end_time = if let Some(dur) = args.duration {
        let total_min = start_h * 60 + start_m + dur;
        format!("{:02}:{:02}", total_min / 60, total_min % 60)
    } else {
        format!("{:02}:{:02}", end_h, end_m)
    };

    println!("\n{}", "Booking:".bold());
    println!("  Resource: {}", label);
    println!("  Subject:  {}", subject);
    println!("  When:     {} {}-{}", target_date, start_time, end_time);

    print!("\n{} ", "Confirm? (y/n):".cyan());
    io::stdout().flush()?;

    let mut confirm = String::new();
    io::stdin().read_line(&mut confirm)?;
    if !confirm.trim().eq_ignore_ascii_case("y") {
        println!("Cancelled.");
        return Ok(());
    }

    // Create the calendar event with the resource as location
    let payload = serde_json::json!({
        "subject": subject,
        "start": book_start,
        "end": book_end,
        "location": label,
        "required_attendees": [email],
    });

    let result = client
        .calendar_invite(&account, payload)
        .map_err(|e| anyhow!("{e}"))?;

    println!("{} Booked {} for \"{}\" on {} {}-{}",
        "+".green().bold(), label, subject, target_date, start_time, end_time);

    debug!("Calendar invite result: {:?}", result);

    Ok(())
}

/// Book a specific resource by alias (programmatic path).
fn book_resource(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    entries: &[Value],
    alias: &str,
    subject: &str,
    window_start: &str,
    window_end: &str,
    duration: Option<u32>,
) -> Result<()> {
    // Find the resource
    let entry = entries
        .iter()
        .find(|e| {
            e["alias"]
                .as_str()
                .map(|a| a.eq_ignore_ascii_case(alias))
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            let available: Vec<&str> = entries
                .iter()
                .filter_map(|e| e["alias"].as_str())
                .collect();
            anyhow!(
                "Unknown resource '{}'. Available: {}",
                alias,
                available.join(", ")
            )
        })?;

    if entry["available"].as_bool() != Some(true) {
        let desc = entry["desc"].as_str();
        let label = if let Some(d) = desc { format!("{} ({})", alias, d) } else { alias.to_string() };
        return Err(anyhow!("{} is not available in this time window", label));
    }

    let email = entry["email"].as_str().unwrap_or("?");
    let desc = entry["desc"].as_str();
    let label = if let Some(d) = desc { format!("{} ({})", alias, d) } else { alias.to_string() };

    // Calculate end time if duration specified
    let book_end = if let Some(dur) = duration {
        // Parse window_start to adjust
        if let Ok(start_dt) = DateTime::parse_from_rfc3339(&format!("{}+01:00", window_start)) {
            (start_dt + ChronoDuration::minutes(dur as i64)).to_rfc3339()
        } else {
            window_end.to_string()
        }
    } else {
        window_end.to_string()
    };

    let payload = serde_json::json!({
        "subject": subject,
        "start": window_start,
        "end": book_end,
        "location": label,
        "required_attendees": [email],
    });

    let result = client
        .calendar_invite(account, payload)
        .map_err(|e| anyhow!("{e}"))?;

    if ctx.common.json || ctx.common.yaml {
        let output = serde_json::json!({
            "booked": true,
            "resource": alias,
            "email": email,
            "subject": subject,
            "start": window_start,
            "end": book_end,
            "result": result,
        });
        emit_output(&ctx.common, &output)?;
    } else {
        use owo_colors::OwoColorize;
        println!(
            "{} Booked {} for \"{}\"",
            "+".green().bold(),
            label,
            subject
        );
    }

    Ok(())
}

/// Capitalize the first letter of a string.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().to_string() + chars.as_str(),
    }
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
            } else if args.interactive && io::stdout().is_terminal() {
                // Interactive mode: let user select a slot and create meeting
                let label = display_names.join(", ");
                let slots: Vec<FreeSlotItem> =
                    serde_json::from_value(result.clone()).context("parsing free slots")?;
                if slots.is_empty() {
                    println!("No common free slots found for {}", label);
                    return Ok(());
                }
                interactive_schedule_meeting(ctx, &account, &client, &label, &slots, &resolved_emails)?;
            } else {
                let label = display_names.join(", ");
                render_free_slots_for_person(&label, &result, ctx, view)?;
            }
        }
        PplCommand::Schedule(args) => {
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

            // Fetch common free slots
            let result = client
                .ppl_common(&account, &email_refs, args.weeks, args.duration, args.limit)
                .map_err(|e| anyhow!("{e}"))?;

            let slots: Vec<FreeSlotItem> =
                serde_json::from_value(result.clone()).context("parsing free slots")?;

            if slots.is_empty() {
                if ctx.common.json || ctx.common.yaml {
                    emit_output(&ctx.common, &serde_json::json!({
                        "slots": [],
                        "people": display_names,
                        "emails": resolved_emails,
                    }))?;
                } else {
                    println!("No common free slots found for {}", display_names.join(", "));
                }
                return Ok(());
            }

            // Build numbered slot list
            let mut numbered_slots: Vec<serde_json::Value> = Vec::new();
            for (i, slot) in slots.iter().enumerate() {
                let start = slot.start.as_deref().unwrap_or("");
                let end = slot.end.as_deref().unwrap_or("");
                let date = slot.date.clone().unwrap_or_else(|| {
                    if start.len() >= 10 { start[..10].to_string() } else { String::new() }
                });
                let start_time = extract_time(start).unwrap_or_default();
                let end_time = extract_time(end).unwrap_or_default();
                let duration = slot.duration_minutes.unwrap_or(0);

                numbered_slots.push(serde_json::json!({
                    "slot": i + 1,
                    "date": date,
                    "start": start,
                    "end": end,
                    "start_time": start_time,
                    "end_time": end_time,
                    "duration_minutes": duration,
                }));
            }

            // If --slot is not provided, just list the available slots
            if args.slot.is_none() {
                if ctx.common.json || ctx.common.yaml {
                    emit_output(&ctx.common, &serde_json::json!({
                        "slots": numbered_slots,
                        "people": display_names,
                        "emails": resolved_emails,
                    }))?;
                } else {
                    let label = display_names.join(", ");
                    println!("Common free slots for: {}\n", label);
                    for ns in &numbered_slots {
                        let num = ns["slot"].as_u64().unwrap_or(0);
                        let date = ns["date"].as_str().unwrap_or("");
                        let st = ns["start_time"].as_str().unwrap_or("");
                        let et = ns["end_time"].as_str().unwrap_or("");
                        let dur = ns["duration_minutes"].as_i64().unwrap_or(0);
                        println!("  [{}] {} {}-{} ({}m)", num, date, st, et, dur);
                    }
                    println!("\nTo book a slot:");
                    let people_str = args.people.join(" ");
                    println!("  h8 ppl schedule {} -w {} --slot N -s \"Subject\" -m MINUTES", people_str, args.weeks);
                }
                return Ok(());
            }

            // --slot provided: create the meeting
            let slot_idx = args.slot.unwrap();
            if slot_idx == 0 || slot_idx > slots.len() {
                return Err(anyhow!("Invalid slot number {}. Valid range: 1-{}", slot_idx, slots.len()));
            }

            let selected = &slots[slot_idx - 1];
            let start = selected.start.as_deref()
                .ok_or_else(|| anyhow!("Selected slot has no start time"))?;
            let max_duration = selected.duration_minutes.unwrap_or(60);

            let subject = args.subject.as_deref()
                .ok_or_else(|| anyhow!("--subject/-s is required when using --slot"))?;

            let meeting_duration = args.meeting_duration.unwrap_or(30.min(max_duration));
            if meeting_duration > max_duration {
                return Err(anyhow!(
                    "Meeting duration {}m exceeds slot maximum of {}m",
                    meeting_duration, max_duration
                ));
            }
            if meeting_duration <= 0 {
                return Err(anyhow!("Meeting duration must be positive"));
            }

            // Calculate end time
            let meeting_start = DateTime::parse_from_rfc3339(start)
                .map_err(|e| anyhow!("Invalid start time '{}': {}", start, e))?;
            let meeting_end = meeting_start + ChronoDuration::minutes(meeting_duration);

            // Build invite payload
            let mut payload = serde_json::json!({
                "subject": subject,
                "start": start,
                "end": meeting_end.to_rfc3339(),
                "required_attendees": resolved_emails,
            });

            if let Some(ref loc) = args.location {
                payload["location"] = serde_json::json!(loc);
            }
            if let Some(ref body) = args.body {
                payload["body"] = serde_json::json!(body);
            }

            let invite_result = client
                .calendar_invite(&account, payload)
                .map_err(|e| anyhow!("{e}"))?;

            if ctx.common.json || ctx.common.yaml {
                emit_output(&ctx.common, &invite_result)?;
            } else {
                let start_time = extract_time(start).unwrap_or_default();
                let end_time_str = extract_time(&meeting_end.to_rfc3339())
                    .unwrap_or_else(|| meeting_end.format("%H:%M").to_string());
                println!("Meeting created: {}", subject);
                println!("  When: {} {}-{} ({}m)", &start[..10], start_time, end_time_str, meeting_duration);
                println!("  With: {}", display_names.join(", "));
                if let Some(ref loc) = args.location {
                    println!("  Where: {}", loc);
                }
            }
        }
        PplCommand::Alias { command } => {
            handle_alias(ctx, command)?;
        }
    }
    Ok(())
}

/// Read the config.toml as a toml_edit Document for in-place editing.
fn read_config_document(ctx: &RuntimeContext) -> Result<(toml_edit::DocumentMut, PathBuf)> {
    let config_path = ctx.paths.global_config.clone();
    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("reading config: {}", config_path.display()))?;
    let doc: toml_edit::DocumentMut = content
        .parse()
        .map_err(|e| anyhow!("parsing config.toml: {e}"))?;
    Ok((doc, config_path))
}

/// Write a toml_edit Document back to disk.
fn write_config_document(doc: &toml_edit::DocumentMut, path: &std::path::Path) -> Result<()> {
    fs::write(path, doc.to_string())
        .with_context(|| format!("writing config: {}", path.display()))
}

/// Handle alias subcommands.
fn handle_alias(ctx: &RuntimeContext, cmd: AliasCommand) -> Result<()> {
    use owo_colors::OwoColorize;

    match cmd {
        AliasCommand::List => {
            if ctx.config.people.is_empty() {
                println!("No aliases configured.");
                println!("Add one with: h8 ppl alias add <name> <email>");
                return Ok(());
            }

            if ctx.common.json || ctx.common.yaml {
                emit_output(&ctx.common, &ctx.config.people)?;
                return Ok(());
            }

            // Sort by name for display
            let mut entries: Vec<_> = ctx.config.people.iter().collect();
            entries.sort_by_key(|(k, _)| k.to_lowercase());

            println!("{}", "Person Aliases".bold());
            println!("{}", "\u{2500}".repeat(50));
            for (name, email) in &entries {
                println!("  {:<16} {}", name.green(), email);
            }
            println!("\n{} alias(es) configured", entries.len());
        }
        AliasCommand::Add(args) => {
            let (mut doc, config_path) = read_config_document(ctx)?;

            // Ensure [people] table exists
            if doc.get("people").is_none() {
                doc["people"] = toml_edit::Item::Table(toml_edit::Table::new());
            }

            let name_lower = args.name.to_lowercase();

            // Check for existing value
            let existing_email = doc["people"]
                .as_table()
                .and_then(|t| t.get(&name_lower))
                .and_then(|v| v.as_str())
                .map(String::from);

            if let Some(ref existing) = existing_email {
                if existing == &args.email {
                    println!("Alias '{}' already set to {}", name_lower, args.email);
                    return Ok(());
                }
            }

            // Set the value
            let people = doc["people"]
                .as_table_mut()
                .ok_or_else(|| anyhow!("[people] is not a table in config.toml"))?;
            people[&name_lower] = toml_edit::value(&args.email);
            write_config_document(&doc, &config_path)?;

            if let Some(existing) = existing_email {
                println!("Updated: {} -> {} (was: {})", name_lower, args.email, existing);
            } else {
                println!("Added: {} -> {}", name_lower, args.email);
            }
        }
        AliasCommand::Remove(args) => {
            let (mut doc, config_path) = read_config_document(ctx)?;

            let people = doc["people"]
                .as_table_mut()
                .ok_or_else(|| anyhow!("[people] section not found in config.toml"))?;

            let name_lower = args.name.to_lowercase();

            // Case-insensitive search for the key
            let key_to_remove = people
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&name_lower))
                .map(|(k, _)| k.to_string());

            if let Some(key) = key_to_remove {
                let email = people[&key].as_str().unwrap_or("?").to_string();
                people.remove(&key);
                write_config_document(&doc, &config_path)?;
                println!("Removed: {} (was: {})", key, email);
            } else {
                return Err(anyhow!("alias '{}' not found", args.name));
            }
        }
        AliasCommand::Search(args) => {
            let account = effective_account(ctx);
            let db_path = ctx.paths.sync_db_path(&account);

            if !db_path.exists() {
                return Err(anyhow!("no address cache - run 'h8 mail sync' first"));
            }

            let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
            let addresses = db
                .search_addresses(Some(&args.query).unwrap(), args.limit)
                .map_err(|e| anyhow!("{e}"))?;

            if addresses.is_empty() {
                println!("No addresses found matching \"{}\"", args.query);
                return Ok(());
            }

            if ctx.common.json || ctx.common.yaml {
                emit_output(&ctx.common, &serde_json::to_value(&addresses)?)?;
                return Ok(());
            }

            // Show results, highlight which ones already have aliases
            let existing: std::collections::HashMap<String, String> = ctx
                .config
                .people
                .iter()
                .map(|(k, v)| (v.to_lowercase(), k.clone()))
                .collect();

            for addr in &addresses {
                let name_part = addr.name.as_deref().unwrap_or("");
                let alias_tag = if let Some(alias) = existing.get(&addr.email.to_lowercase()) {
                    format!(" [alias: {}]", alias.green())
                } else {
                    String::new()
                };

                if name_part.is_empty() {
                    println!("  {}{}", addr.email, alias_tag);
                } else {
                    println!("  {} <{}>{}", name_part, addr.email, alias_tag);
                }
            }
        }
        AliasCommand::Pick(args) => {
            if !io::stdout().is_terminal() {
                return Err(anyhow!("pick requires an interactive terminal"));
            }

            let account = effective_account(ctx);
            let db_path = ctx.paths.sync_db_path(&account);

            if !db_path.exists() {
                return Err(anyhow!("no address cache - run 'h8 mail sync' first"));
            }

            let db = Database::open(&db_path).map_err(|e| anyhow!("{e}"))?;
            let addresses = if args.frequent {
                db.frequent_addresses(args.limit).map_err(|e| anyhow!("{e}"))?
            } else {
                db.frequent_addresses(args.limit).map_err(|e| anyhow!("{e}"))?
            };

            if addresses.is_empty() {
                println!("No cached addresses. Run 'h8 mail sync' first.");
                return Ok(());
            }

            // Build existing alias lookup (email -> alias)
            let existing_aliases: std::collections::HashMap<String, String> = ctx
                .config
                .people
                .iter()
                .map(|(k, v)| (v.to_lowercase(), k.clone()))
                .collect();

            // Filter out addresses that already have aliases
            let unaliased: Vec<_> = addresses
                .iter()
                .filter(|a| !existing_aliases.contains_key(&a.email.to_lowercase()))
                .collect();

            if unaliased.is_empty() {
                println!("All frequently used addresses already have aliases.");
                return Ok(());
            }

            // Build display items for the multi-select
            let items: Vec<String> = unaliased
                .iter()
                .map(|addr| {
                    let name_part = addr.name.as_deref().unwrap_or("");
                    let total = addr.send_count + addr.receive_count;
                    if name_part.is_empty() {
                        format!("{:<45} ({} msgs)", addr.email, total)
                    } else {
                        format!("{} <{}>  ({} msgs)", name_part, addr.email, total)
                    }
                })
                .collect();

            // Multi-select with arrow keys + space
            let selections = dialoguer::MultiSelect::new()
                .with_prompt("Select contacts to alias (Space to toggle, Enter to confirm)")
                .items(&items)
                .interact_opt()
                .map_err(|e| anyhow!("selection cancelled: {e}"))?;

            let selections = match selections {
                Some(s) if !s.is_empty() => s,
                _ => {
                    println!("No contacts selected.");
                    return Ok(());
                }
            };

            // Now prompt for alias names for each selected address
            let (mut doc, config_path) = read_config_document(ctx)?;

            // Ensure [people] table exists
            if doc.get("people").is_none() {
                doc["people"] = toml_edit::Item::Table(toml_edit::Table::new());
            }

            let mut added = 0;

            println!();
            for &idx in &selections {
                let addr = &unaliased[idx];
                let suggested = suggest_alias(addr);

                let prompt = if addr.name.is_some() {
                    format!("Alias for {} <{}>", addr.name.as_deref().unwrap_or(""), addr.email)
                } else {
                    format!("Alias for {}", addr.email)
                };

                let alias = dialoguer::Input::<String>::new()
                    .with_prompt(&prompt)
                    .default(suggested)
                    .interact_text()
                    .map_err(|e| anyhow!("input cancelled: {e}"))?;

                let alias = alias.trim().to_lowercase();

                if alias.is_empty() {
                    println!("  Skipped {}", addr.email);
                    continue;
                }

                let people = doc["people"]
                    .as_table_mut()
                    .ok_or_else(|| anyhow!("[people] is not a table"))?;

                if people.contains_key(&alias) {
                    let existing_val = people[&alias].as_str().unwrap_or("?");
                    println!(
                        "  Alias '{}' already exists (-> {}), skipping.",
                        alias, existing_val
                    );
                    continue;
                }

                people[&alias] = toml_edit::value(&addr.email);
                added += 1;
                println!("  {} {} -> {}", "+".green().bold(), alias.green(), addr.email);
            }

            if added > 0 {
                write_config_document(&doc, &config_path)?;
                println!("\nSaved {} new alias(es) to {}", added, config_path.display());
            } else {
                println!("\nNo aliases added.");
            }
        }
    }

    Ok(())
}

/// Suggest an alias name from an address entry.
fn suggest_alias(addr: &h8_core::types::AddressEntry) -> String {
    // Try to derive from display name
    if let Some(ref name) = addr.name {
        let name = name.trim();
        if !name.is_empty() {
            // Use first name, lowercased
            let first = name.split_whitespace().next().unwrap_or(name);
            let clean: String = first
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-')
                .collect();
            if !clean.is_empty() {
                return clean.to_lowercase();
            }
        }
    }

    // Fall back to email local part
    if let Some(local) = addr.email.split('@').next() {
        let clean: String = local
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '.')
            .collect();
        if !clean.is_empty() {
            return clean.to_lowercase();
        }
    }

    String::new()
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

/// Interactive meeting scheduling from common free slots.
fn interactive_schedule_meeting(
    ctx: &RuntimeContext,
    account: &str,
    client: &ServiceClient,
    label: &str,
    slots: &[FreeSlotItem],
    attendee_emails: &[String],
) -> Result<()> {
    use owo_colors::OwoColorize;
    use std::io::{self, Write};

    if slots.is_empty() {
        println!("No common free slots found for {}", label);
        return Ok(());
    }

    let tz = ctx
        .config
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::UTC);

    // Build a flat list of valid slots for selection
    let mut selectable_slots: Vec<(String, String, i64, String)> = Vec::new(); // (start, end, duration, display)
    let today = Local::now().with_timezone(&tz).date_naive();

    // Group by date for display
    let mut slots_by_date: std::collections::BTreeMap<String, Vec<&FreeSlotItem>> =
        std::collections::BTreeMap::new();

    for item in slots {
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

    println!("\n{}", format!("Common free slots for: {}", label).bold());
    println!("{}", "\u{2500}".repeat(60));

    let mut slot_num = 0;
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

        println!("\n  {}", date_label.cyan().bold());

        for slot in day_slots {
            if let (Some(start), Some(end)) = (&slot.start, &slot.end) {
                let start_time = extract_time(start).unwrap_or_else(|| "??:??".to_string());
                let end_time = extract_time(end).unwrap_or_else(|| "??:??".to_string());
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

                slot_num += 1;
                selectable_slots.push((
                    start.clone(),
                    end.clone(),
                    duration,
                    format!("{} {}-{} ({})" , date_str, start_time, end_time, duration_str),
                ));

                println!(
                    "    [{}] {}-{}  {}",
                    slot_num.to_string().yellow().bold(),
                    start_time,
                    end_time,
                    duration_str.dimmed()
                );
            }
        }
    }

    if selectable_slots.is_empty() {
        println!("\n(no valid free slots found)");
        return Ok(());
    }

    // Prompt user to select
    print!("\n{} ", "Select slot (1-{}), or 'c' to cancel:".replace("{}", &selectable_slots.len().to_string()).cyan());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.eq_ignore_ascii_case("c") || input.eq_ignore_ascii_case("cancel") {
        println!("Cancelled.");
        return Ok(());
    }

    let selection: usize = match input.parse::<usize>() {
        Ok(n) if n > 0 && n <= selectable_slots.len() => n - 1,
        _ => {
            println!("Invalid selection.");
            return Ok(());
        }
    };

    let (start, _end, max_duration, _display) = &selectable_slots[selection];

    // Ask for duration
    print!("{} ", format!("Duration in minutes (max {}m, default 30):", max_duration).cyan());
    io::stdout().flush()?;

    let mut duration_input = String::new();
    io::stdin().read_line(&mut duration_input)?;
    let duration_input = duration_input.trim();

    let duration: i64 = if duration_input.is_empty() {
        30
    } else {
        match duration_input.parse::<i64>() {
            Ok(n) if n > 0 && n <= *max_duration => n,
            Ok(n) if n > *max_duration => {
                println!("Duration too long, using max: {}m", max_duration);
                *max_duration
            }
            _ => {
                println!("Invalid duration, using default: 30m");
                30
            }
        }
    };

    // Calculate meeting end time based on duration
    let meeting_start = DateTime::parse_from_rfc3339(start)
        .map_err(|e| anyhow!("Invalid start time: {}", e))?;
    let meeting_end = meeting_start + ChronoDuration::minutes(duration);

    // Ask for subject
    print!("{} ", "Meeting subject:".cyan());
    io::stdout().flush()?;

    let mut subject = String::new();
    io::stdin().read_line(&mut subject)?;
    let subject = subject.trim();

    if subject.is_empty() {
        println!("Cancelled - subject required.");
        return Ok(());
    }

    // Confirm
    let start_time = extract_time(start).unwrap_or_else(|| start.clone());
    let end_time_str = extract_time(&meeting_end.to_rfc3339()).unwrap_or_else(|| meeting_end.format("%H:%M").to_string());

    println!("\n{}", "Meeting Details:".bold());
    println!("  Subject: {}", subject);
    println!("  When: {} {}-{}", &start[..10], start_time, end_time_str);
    println!("  Duration: {}m", duration);
    println!("  Attendees: {}", label);

    print!("\n{} ", "Create meeting? (y/n):".cyan());
    io::stdout().flush()?;

    let mut confirm = String::new();
    io::stdin().read_line(&mut confirm)?;

    if !confirm.trim().eq_ignore_ascii_case("y") && !confirm.trim().eq_ignore_ascii_case("yes") {
        println!("Cancelled.");
        return Ok(());
    }

    // Create the meeting invite
    let payload = serde_json::json!({
        "subject": subject,
        "start": start,
        "end": meeting_end.to_rfc3339(),
        "required_attendees": attendee_emails,
    });

    let result = client.calendar_invite(account, payload)?;

    println!("\n{}", "Meeting created!".green().bold());

    // Display attendees from response if available
    if let Some(req) = result.get("required_attendees").and_then(|v| v.as_array()) {
        if !req.is_empty() {
            let emails: Vec<&str> = req.iter().filter_map(|v| v.as_str()).collect();
            if !emails.is_empty() {
                println!("  Invites sent to: {}", emails.join(", "));
            }
        }
    }

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

// === Rules Handlers ===

/// Open the database for the account.
fn open_database(ctx: &RuntimeContext, account: &str) -> Result<Database> {
    let db_path = ctx.paths.sync_db_path(account);
    Database::open(&db_path).map_err(|e| anyhow!("{e}"))
}

/// Resolve a rule ID to its remote ID.
/// If the input looks like a short ID (contains a hyphen, not a GUID), look it up in the database.
/// Otherwise, assume it's already a remote ID.
fn resolve_rule_id(id_gen: &IdGenerator, id: &str) -> Result<String> {
    // If it looks like a GUID (contains braces or is 32+ chars with hyphens), use directly
    if id.starts_with("{") || (id.len() > 30 && id.chars().filter(|&c| c == '-').count() >= 3) {
        return Ok(id.to_string());
    }

    // Try to resolve as a short ID
    if let Some(remote_id) = id_gen.resolve_rule(id).map_err(|e| anyhow!("{e}"))? {
        return Ok(remote_id);
    }

    // Not found - assume it's a remote ID
    Ok(id.to_string())
}

fn handle_rules(ctx: &RuntimeContext, command: RulesCommand) -> Result<()> {
    let client = ctx.service_client()?;
    let account = effective_account(ctx);

    match command {
        RulesCommand::List(args) => {
            let db = open_database(ctx, &account)?;
            let id_gen = IdGenerator::new(&db);
            let result = client.rules_list(&account).map_err(|e| anyhow!("{e}"))?;
            let rules_with_ids = assign_rule_ids(&result, &id_gen)?;
            if ctx.common.json {
                emit_output(&ctx.common, &rules_with_ids)?;
            } else {
                print_rules_list(&rules_with_ids, args.detailed);
            }
        }
        RulesCommand::Show(args) => {
            let db = open_database(ctx, &account)?;
            let id_gen = IdGenerator::new(&db);
            let remote_id = resolve_rule_id(&id_gen, &args.id)?;
            let result = client.rules_get(&account, &remote_id).map_err(|e| anyhow!("{e}"))?;
            let rule_with_id = assign_rule_id_to_single(&result, &id_gen)?;
            emit_output(&ctx.common, &rule_with_id)?;
        }
        RulesCommand::Create(args) => handle_rules_create(ctx, &client, &account, args)?,
        RulesCommand::Enable(args) => {
            let db = open_database(ctx, &account)?;
            let id_gen = IdGenerator::new(&db);
            let remote_id = resolve_rule_id(&id_gen, &args.id)?;
            let result = client.rules_enable(&account, &remote_id).map_err(|e| anyhow!("{e}"))?;
            let short_id = get_rule_short_id(&result, &id_gen);
            if !ctx.common.json {
                println!("Rule enabled: {}", short_id);
            }
            emit_output(&ctx.common, &result)?;
        }
        RulesCommand::Disable(args) => {
            let db = open_database(ctx, &account)?;
            let id_gen = IdGenerator::new(&db);
            let remote_id = resolve_rule_id(&id_gen, &args.id)?;
            let result = client.rules_disable(&account, &remote_id).map_err(|e| anyhow!("{e}"))?;
            let short_id = get_rule_short_id(&result, &id_gen);
            if !ctx.common.json {
                println!("Rule disabled: {}", short_id);
            }
            emit_output(&ctx.common, &result)?;
        }
        RulesCommand::Delete(args) => {
            let db = open_database(ctx, &account)?;
            let id_gen = IdGenerator::new(&db);
            let remote_id = resolve_rule_id(&id_gen, &args.id)?;
            if !args.yes && !ctx.common.assume_yes {
                print!("Delete rule {}? [y/N] ", args.id);
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Cancelled");
                    return Ok(());
                }
            }
            let result = client.rules_delete(&account, &remote_id).map_err(|e| anyhow!("{e}"))?;
            let _ = id_gen.delete_rule(&args.id);
            if !ctx.common.json {
                println!("Rule deleted: {}", args.id);
            }
            emit_output(&ctx.common, &result)?;
        }
    }

    Ok(())
}

/// Assign readable IDs to all rules in the list.
fn assign_rule_ids(rules: &Value, id_gen: &IdGenerator) -> Result<Value> {
    let rules_array = match rules.as_array() {
        Some(arr) => arr,
        None => return Ok(rules.clone()),
    };

    let mut result = Vec::new();
    for rule in rules_array {
        let remote_id = rule.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let display_name = rule.get("display_name").and_then(|v| v.as_str());

        // Get or create a short ID for this rule
        let short_id = id_gen
            .get_or_create_rule_id(remote_id, display_name)
            .map_err(|e| anyhow!("{e}"))?;

        // Create a new rule object with the short ID
        let mut rule_with_id = rule.clone();
        if let Some(obj) = rule_with_id.as_object_mut() {
            obj.insert("id".to_string(), json!(short_id));
            obj.insert("remote_id".to_string(), json!(remote_id));
        }
        result.push(rule_with_id);
    }

    Ok(json!(result))
}

/// Assign readable ID to a single rule.
fn assign_rule_id_to_single(rule: &Value, id_gen: &IdGenerator) -> Result<Value> {
    let remote_id = rule.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let display_name = rule.get("display_name").and_then(|v| v.as_str());

    let short_id = id_gen
        .get_or_create_rule_id(remote_id, display_name)
        .map_err(|e| anyhow!("{e}"))?;

    let mut rule_with_id = rule.clone();
    if let Some(obj) = rule_with_id.as_object_mut() {
        obj.insert("id".to_string(), json!(short_id));
        obj.insert("remote_id".to_string(), json!(remote_id));
    }

    Ok(rule_with_id)
}

/// Get the short ID for a rule from the result.
fn get_rule_short_id(rule: &Value, id_gen: &IdGenerator) -> String {
    let remote_id = rule.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let display_name = rule.get("display_name").and_then(|v| v.as_str());

    // Try to get existing short ID, otherwise create one
    id_gen
        .get_or_create_rule_id(remote_id, display_name)
        .unwrap_or_else(|_| remote_id.to_string())
}

fn print_rules_list(result: &Value, verbose: bool) {
    use owo_colors::OwoColorize;

    let empty_vec = vec![];
    let rules = result.as_array().unwrap_or(&empty_vec);
    if rules.is_empty() {
        println!("No inbox rules configured");
        return;
    }

    if verbose {
        for rule in rules {
            let id = rule.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
            let name = rule.get("display_name").and_then(|v| v.as_str()).unwrap_or("Untitled");
            let priority = rule.get("priority").and_then(|v| v.as_i64()).unwrap_or(0);
            let enabled = rule.get("is_enabled").and_then(|v| v.as_bool()).unwrap_or(true);

            let status_str = if enabled {
                "enabled".green().to_string()
            } else {
                "disabled".dimmed().to_string()
            };

            println!("{}  {}  (priority: {}, {})",
                id,
                name.bold(),
                priority,
                status_str
            );

            if let Some(conditions) = rule.get("conditions") {
                println!("  Conditions:");
                if let Some(obj) = conditions.as_object() {
                    for (key, val) in obj {
                        println!("    - {}: {}", key, format_json_value(val).dimmed());
                    }
                }
            }

            if let Some(actions) = rule.get("actions") {
                println!("  Actions:");
                if let Some(obj) = actions.as_object() {
                    for (key, val) in obj {
                        println!("    - {}: {}", key, format_json_value(val).dimmed());
                    }
                }
            }
            println!();
        }
    } else {
        // Compact table view
        println!("{:<12} {:<6} {:<10} {}", "ID", "Prio", "Status", "Name");
        println!("{}", "-".repeat(60));
        for rule in rules {
            let id = rule.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
            let short_id = if id.len() > 10 { &id[..10] } else { id };
            let name = rule.get("display_name").and_then(|v| v.as_str()).unwrap_or("Untitled");
            let priority = rule.get("priority").and_then(|v| v.as_i64()).unwrap_or(0);
            let enabled = rule.get("is_enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            let status = if enabled { "on".green().to_string() } else { "off".dimmed().to_string() };

            let display_name = if name.len() > 30 { format!("{}...", &name[..27]) } else { name.to_string() };
            println!("{:<12} {:<6} {:<10} {}", short_id, priority, status, display_name);
        }
    }
}

fn format_json_value(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_json_value).collect();
            format!("[{}]", items.join(", "))
        }
        _ => val.to_string(),
    }
}

fn handle_rules_create(
    ctx: &RuntimeContext,
    client: &ServiceClient,
    account: &str,
    args: RulesCreateArgs,
) -> Result<()> {
    let db = open_database(ctx, account)?;
    let id_gen = IdGenerator::new(&db);

    // Build the rule name from positional args if it's a natural language query
    let name = args.name.join(" ");

    // Build conditions and actions from flags
    let mut conditions: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut actions: serde_json::Map<String, Value> = serde_json::Map::new();

    if let Some(from) = &args.from {
        conditions.insert("from_addresses".to_string(), json!([from]));
    }
    if let Some(subject) = &args.subject_contains {
        conditions.insert("contains_subject_strings".to_string(), json!([subject]));
    }
    if let Some(body) = &args.body_contains {
        conditions.insert("contains_body_strings".to_string(), json!([body]));
    }
    if args.has_attachments {
        conditions.insert("has_attachments".to_string(), json!(true));
    }

    if let Some(folder) = &args.move_to {
        actions.insert("move_to_folder".to_string(), json!(folder));
    }
    if let Some(folder) = &args.copy_to {
        actions.insert("copy_to_folder".to_string(), json!(folder));
    }
    if args.delete {
        actions.insert("delete".to_string(), json!(true));
    }
    if args.mark_read {
        actions.insert("mark_as_read".to_string(), json!(true));
    }
    if let Some(forward) = &args.forward_to {
        actions.insert("forward_to_recipients".to_string(), json!([forward]));
    }

    // If no explicit conditions/actions were given, try to parse natural language
    let (parsed_name, parsed_conditions, parsed_actions) = if conditions.is_empty() && actions.is_empty() {
        parse_natural_rule(&name)
    } else {
        (name, conditions, actions)
    };

    if parsed_conditions.is_empty() {
        return Err(anyhow!("No conditions specified. Use --from, --subject-contains, etc. or provide natural language"));
    }
    if parsed_actions.is_empty() {
        return Err(anyhow!("No actions specified. Use --move-to, --delete, --mark-read, etc."));
    }

    let payload = json!({
        "display_name": parsed_name,
        "priority": args.priority,
        "is_enabled": args.enabled,
        "conditions": parsed_conditions,
        "actions": parsed_actions,
    });

    let result = client.rules_create(account, payload).map_err(|e| anyhow!("{e}"))?;

    // Assign a readable ID to the newly created rule
    let result_with_id = assign_rule_id_to_single(&result, &id_gen)?;

    if !ctx.common.json {
        let id = result_with_id.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
        println!("Rule created: {} ({})", id, parsed_name);
    }

    emit_output(&ctx.common, &result_with_id)
}

/// Parse natural language rule descriptions
/// Examples:
///   "move newsletters to Archive if subject contains 'Weekly'"
///   "delete emails from spam@example.com"
fn parse_natural_rule(name: &str) -> (String, serde_json::Map<String, Value>, serde_json::Map<String, Value>) {
    let mut conditions: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut actions: serde_json::Map<String, Value> = serde_json::Map::new();

    let lower = name.to_lowercase();

    // Detect actions
    if lower.contains("move to") || lower.contains("move emails to") {
        // Extract folder name after "to" or "folder"
        if let Some(folder) = extract_folder_name(name) {
            actions.insert("move_to_folder".to_string(), json!(folder));
        }
    }
    if lower.contains("delete") || lower.contains("trash") {
        actions.insert("delete".to_string(), json!(true));
    }
    if lower.contains("mark as read") || lower.contains("mark read") {
        actions.insert("mark_as_read".to_string(), json!(true));
    }
    if lower.contains("copy to") || lower.contains("copy emails to") {
        if let Some(folder) = extract_folder_name(name) {
            actions.insert("copy_to_folder".to_string(), json!(folder));
        }
    }
    if lower.contains("forward to") {
        // Try to extract email after "forward to"
        if let Some(email) = extract_email_after(name, "forward to") {
            actions.insert("forward_to_recipients".to_string(), json!([email]));
        }
    }

    // Detect conditions
    if lower.contains("if subject contains") || lower.contains("subject contains") {
        if let Some(text) = extract_quoted_or_after(name, "contains") {
            conditions.insert("contains_subject_strings".to_string(), json!([text]));
        }
    }
    if lower.contains("if body contains") || lower.contains("body contains") {
        if let Some(text) = extract_quoted_or_after(name, "contains") {
            conditions.insert("contains_body_strings".to_string(), json!([text]));
        }
    }
    if lower.contains("from ") || lower.contains("if from ") {
        if let Some(email) = extract_email_after(name, "from") {
            conditions.insert("from_addresses".to_string(), json!([email]));
        }
    }
    if lower.contains("has attachments") || lower.contains("with attachments") {
        conditions.insert("has_attachments".to_string(), json!(true));
    }

    (name.to_string(), conditions, actions)
}

fn extract_folder_name(s: &str) -> Option<String> {
    // Look for "to FolderName" or "folder FolderName"
    let lower = s.to_lowercase();
    for pattern in &["move to ", "move emails to ", "copy to ", "copy emails to ", "folder "] {
        if let Some(pos) = lower.find(pattern) {
            let start = pos + pattern.len();
            let rest = &s[start..];
            // Take until next keyword or end
            let end = rest.find(" if ").unwrap_or(rest.len());
            return Some(rest[..end].trim().to_string());
        }
    }
    None
}

fn extract_email_after(s: &str, keyword: &str) -> Option<String> {
    let lower = s.to_lowercase();
    if let Some(pos) = lower.find(&keyword.to_lowercase()) {
        let start = pos + keyword.len();
        let rest = &s[start..].trim_start();
        // Extract what's likely an email or until next word
        let words: Vec<&str> = rest.split_whitespace().collect();
        if !words.is_empty() {
            let email = words[0].trim_matches(|c| c == '\'' || c == '"');
            return Some(email.to_string());
        }
    }
    None
}

fn extract_quoted_or_after(s: &str, keyword: &str) -> Option<String> {
    // First try to extract quoted string
    if let Some(start) = s.find('\'') {
        if let Some(end) = s[start + 1..].find('\'') {
            return Some(s[start + 1..start + 1 + end].to_string());
        }
    }
    if let Some(start) = s.find('"') {
        if let Some(end) = s[start + 1..].find('"') {
            return Some(s[start + 1..start + 1 + end].to_string());
        }
    }
    // Otherwise extract word after keyword
    extract_email_after(s, keyword)
}

// === OOF Handlers ===

fn handle_oof(ctx: &RuntimeContext, command: OofCommand) -> Result<()> {
    let client = ctx.service_client()?;
    let account = effective_account(ctx);

    match command {
        OofCommand::Status => {
            let result = client.oof_get(&account).map_err(|e| anyhow!("{e}"))?;
            if ctx.common.json {
                emit_output(&ctx.common, &result)?;
            } else {
                print_oof_status(&result);
            }
        }
        OofCommand::Enable(args) => {
            let internal = args.message.join(" ");
            let external = args.external.as_deref();
            let result = client
                .oof_enable(&account, &internal, external, &args.audience)
                .map_err(|e| anyhow!("{e}"))?;
            if !ctx.common.json {
                println!("Out-of-Office enabled");
            }
            emit_output(&ctx.common, &result)?;
        }
        OofCommand::Schedule(args) => {
            // Parse natural language dates if needed
            let start = parse_datetime_natural(&args.start)?;
            let end = parse_datetime_natural(&args.end)?;
            let result = client
                .oof_schedule(
                    &account,
                    &start,
                    &end,
                    &args.message,
                    args.external.as_deref(),
                    &args.audience,
                )
                .map_err(|e| anyhow!("{e}"))?;
            if !ctx.common.json {
                println!("Out-of-Office scheduled");
            }
            emit_output(&ctx.common, &result)?;
        }
        OofCommand::Disable => {
            let result = client.oof_disable(&account).map_err(|e| anyhow!("{e}"))?;
            if !ctx.common.json {
                println!("Out-of-Office disabled");
            }
            emit_output(&ctx.common, &result)?;
        }
    }

    Ok(())
}

fn print_oof_status(result: &Value) {
    use owo_colors::OwoColorize;

    let _state = result.get("state").and_then(|v| v.as_str()).unwrap_or("Unknown");
    let enabled = result.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    let scheduled = result.get("scheduled").and_then(|v| v.as_bool()).unwrap_or(false);
    let audience = result.get("external_audience").and_then(|v| v.as_str()).unwrap_or("Unknown");

    if enabled {
        println!("Out-of-Office: {}", "ENABLED".green().bold());
        if scheduled {
            if let Some(start) = result.get("start").and_then(|v| v.as_str()) {
                if let Some(end) = result.get("end").and_then(|v| v.as_str()) {
                    println!("Scheduled: {} to {}", start, end);
                }
            }
        } else {
            println!("Active now (not scheduled)");
        }
        println!("External audience: {}", audience);

        if let Some(reply) = result.get("internal_reply").and_then(|v| v.as_str()) {
            if !reply.is_empty() {
                println!("\nInternal reply:");
                println!("  {}", reply);
            }
        }
        if let Some(reply) = result.get("external_reply").and_then(|v| v.as_str()) {
            if !reply.is_empty() {
                println!("\nExternal reply:");
                println!("  {}", reply);
            }
        }
    } else {
        println!("Out-of-Office: {}", "DISABLED".dimmed());
    }
}

fn parse_datetime_natural(input: &str) -> Result<String> {
    // If already ISO format, return as-is
    if input.contains('T') || input.contains('-') && input.len() >= 10 {
        return Ok(input.to_string());
    }

    // Try to parse natural language via service dateparser
    // For now, just pass through and let the service handle it
    Ok(input.to_string())
}

// === Sync Handler ===

fn handle_sync(ctx: &RuntimeContext, args: SyncArgs) -> Result<()> {
    let client = ctx.service_client()?;
    let account = effective_account(ctx);
    let sync_everything = args.sync_everything();

    let mut results = serde_json::Map::new();
    let mut has_errors = false;

    // Sync Calendar
    if sync_everything || args.calendar {
        if !ctx.common.quiet {
            println!("Syncing calendar...");
        }
        match client.calendar_list(
            &account,
            args.weeks * 7,
            None,
            None,
        ) {
            Ok(events) => {
                // Sync to local database
                let db_path = ctx.paths.sync_db_path(&account);
                match Database::open(&db_path) {
                    Ok(db) => {
                        let id_gen = IdGenerator::new(&db);
                        // Ensure ID pool is seeded
                        if let Ok(stats) = id_gen.stats() {
                            if stats.total() == 0 {
                                let words = h8_core::id::WordLists::embedded();
                                let _ = id_gen.init_pool(&words);
                            }
                        }
                        if let Ok(synced) = sync_calendar_events(ctx, &account, &events) {
                            results.insert("calendar".to_string(), json!({
                                "status": "ok",
                                "events_synced": synced.as_array().map(|a| a.len()).unwrap_or(0),
                            }));
                            if !ctx.common.quiet {
                                println!("  ✓ Calendar: {} events synced", synced.as_array().map(|a| a.len()).unwrap_or(0));
                            }
                        } else {
                            has_errors = true;
                            results.insert("calendar".to_string(), json!({
                                "status": "error",
                                "message": "Failed to sync to local database",
                            }));
                        }
                    }
                    Err(e) => {
                        has_errors = true;
                        results.insert("calendar".to_string(), json!({
                            "status": "error",
                            "message": format!("Database error: {}", e),
                        }));
                    }
                }
            }
            Err(e) => {
                has_errors = true;
                results.insert("calendar".to_string(), json!({
                    "status": "error",
                    "message": format!("Service error: {}", e),
                }));
                if !ctx.common.quiet {
                    eprintln!("  ✗ Calendar sync failed: {}", e);
                }
            }
        }
    }

    // Sync Mail — delegate to the same handler as `h8 mail sync`
    if sync_everything || args.mail {
        if !ctx.common.quiet {
            println!("Syncing mail...");
        }
        let mail_args = MailSyncArgs {
            folder: None,
            full: args.full,
            limit_days: args.limit_days,
        };
        match handle_mail_sync(ctx, &client, &account, mail_args) {
            Ok(()) => {
                results.insert("mail".to_string(), json!({ "status": "ok" }));
            }
            Err(e) => {
                has_errors = true;
                results.insert("mail".to_string(), json!({
                    "status": "error",
                    "message": format!("{}", e),
                }));
                if !ctx.common.quiet {
                    eprintln!("  ✗ Mail sync failed: {}", e);
                }
            }
        }
    }

    // Sync Contacts
    if sync_everything || args.contacts {
        if !ctx.common.quiet {
            println!("Syncing contacts...");
        }
        let limit = 1000; // Sync up to 1000 contacts
        match client.contacts_list(&account, limit, None) {
            Ok(contacts) => {
                let count = contacts.as_array().map(|a| a.len()).unwrap_or(0);
                results.insert("contacts".to_string(), json!({
                    "status": "ok",
                    "contacts_synced": count,
                }));
                if !ctx.common.quiet {
                    println!("  ✓ Contacts: {} contacts synced", count);
                }
            }
            Err(e) => {
                has_errors = true;
                results.insert("contacts".to_string(), json!({
                    "status": "error",
                    "message": format!("Service error: {}", e),
                }));
                if !ctx.common.quiet {
                    eprintln!("  ✗ Contacts sync failed: {}", e);
                }
            }
        }
    }

    // Output results
    if ctx.common.json {
        emit_output(&ctx.common, &json!(results))?;
    } else if !ctx.common.quiet {
        if has_errors {
            println!("\nSync completed with errors.");
        } else {
            println!("\nSync completed successfully.");
        }
    }

    if has_errors {
        Err(anyhow!("One or more sync operations failed"))
    } else {
        Ok(())
    }
}
