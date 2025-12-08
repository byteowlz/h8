use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use config::{Config, Environment, File, FileFormat};
use dirs;
use env_logger::fmt::WriteStyle;
use libc;
use log::{LevelFilter, debug};
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shellexpand;

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
    #[command(alias = "ls")]
    List(MailListArgs),
    Get(MailGetArgs),
    Fetch(MailFetchArgs),
    Send(MailSendArgs),
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

#[derive(Debug, Args)]
struct MailSendArgs {
    #[arg(long)]
    file: Option<PathBuf>,
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
        let mut paths = AppPaths::discover(common.config.clone())?;
        ensure_default_config(&paths.global_config)?;
        let config = load_config(&mut paths, &common)?;
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
}

#[derive(Debug, Clone)]
struct AppPaths {
    global_config: PathBuf,
    local_config: PathBuf,
    cli_config: Option<PathBuf>,
    state_dir: PathBuf,
}

impl AppPaths {
    fn discover(cli_config: Option<PathBuf>) -> Result<Self> {
        let global_config = default_config_dir()?.join("config.toml");
        let local_config = env::current_dir()
            .context("determining current directory")?
            .join("config.toml");
        let cli_config = cli_config.map(expand_path).transpose()?;
        let state_dir = default_state_dir()?;

        Ok(Self {
            global_config,
            local_config,
            cli_config,
            state_dir,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct AppConfig {
    account: String,
    timezone: String,
    service_url: String,
    free_slots: FreeSlotsConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            account: "your.email@example.com".to_string(),
            timezone: "Europe/Berlin".to_string(),
            service_url: "http://127.0.0.1:8787".to_string(),
            free_slots: FreeSlotsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct FreeSlotsConfig {
    start_hour: u8,
    end_hour: u8,
    exclude_weekends: bool,
}

impl Default for FreeSlotsConfig {
    fn default() -> Self {
        Self {
            start_hour: 9,
            end_hour: 17,
            exclude_weekends: true,
        }
    }
}

fn handle_calendar(ctx: &RuntimeContext, cmd: CalendarCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ServiceClient::new(ctx)?;
    match cmd {
        CalendarCommand::List(args) => {
            let events = client.calendar_list(
                &account,
                args.days,
                args.from_date.as_deref(),
                args.to_date.as_deref(),
            )?;
            emit_output(&ctx.common, &events)?;
        }
        CalendarCommand::Create(args) => {
            let payload = read_json_payload(args.file.as_ref())?;
            let event = client.calendar_create(&account, payload)?;
            emit_output(&ctx.common, &event)?;
        }
        CalendarCommand::Delete(args) => {
            let result = client.calendar_delete(&account, &args.id, args.change_key.as_deref())?;
            emit_output(&ctx.common, &result)?;
        }
    }
    Ok(())
}

fn handle_mail(ctx: &RuntimeContext, cmd: MailCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ServiceClient::new(ctx)?;
    match cmd {
        MailCommand::List(args) => {
            let messages = client.mail_list(&account, &args.folder, args.limit, args.unread)?;
            emit_output(&ctx.common, &messages)?;
        }
        MailCommand::Get(args) => {
            let message = client.mail_get(&account, &args.folder, &args.id)?;
            emit_output(&ctx.common, &message)?;
        }
        MailCommand::Fetch(args) => {
            let result = client.mail_fetch(
                &account,
                &args.folder,
                &args.output,
                args.format,
                args.limit,
            )?;
            emit_output(&ctx.common, &result)?;
        }
        MailCommand::Send(args) => {
            let payload = read_json_payload(args.file.as_ref())?;
            let result = client.mail_send(&account, payload)?;
            emit_output(&ctx.common, &result)?;
        }
    }
    Ok(())
}

fn handle_contacts(ctx: &RuntimeContext, cmd: ContactsCommand) -> Result<()> {
    let account = effective_account(ctx);
    let client = ServiceClient::new(ctx)?;
    match cmd {
        ContactsCommand::List(args) => {
            let contacts = client.contacts_list(&account, args.limit, args.search.as_deref())?;
            emit_output(&ctx.common, &contacts)?;
        }
        ContactsCommand::Get(args) => {
            let contact = client.contacts_get(&account, &args.id)?;
            emit_output(&ctx.common, &contact)?;
        }
        ContactsCommand::Create(args) => {
            let payload = read_json_payload(args.file.as_ref())?;
            let result = client.contacts_create(&account, payload)?;
            emit_output(&ctx.common, &result)?;
        }
        ContactsCommand::Delete(args) => {
            let result = client.contacts_delete(&account, &args.id)?;
            emit_output(&ctx.common, &result)?;
        }
    }
    Ok(())
}

fn handle_agenda(ctx: &RuntimeContext, args: AgendaArgs) -> Result<()> {
    let account = args.account.unwrap_or_else(|| effective_account(ctx));
    let client = ServiceClient::new(ctx)?;

    // Today range in configured timezone
    let tz = ctx
        .config
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::UTC);
    let today = Local::now().with_timezone(&tz).date_naive();
    let start = today.and_hms_opt(0, 0, 0).unwrap();
    let end = today.and_hms_opt(23, 59, 59).unwrap();

    let events_val = client.calendar_list(
        &account,
        1,
        Some(&start.format("%Y-%m-%dT%H:%M:%S").to_string()),
        Some(&end.format("%Y-%m-%dT%H:%M:%S").to_string()),
    )?;

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
    let client = ServiceClient::new(ctx)?;
    let slots = client.free_slots(&account, cmd.weeks, cmd.duration, cmd.limit)?;
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
        ConfigCommand::Reset => write_default_config(&ctx.paths.global_config),
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
    write_default_config(&ctx.paths.global_config)
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
        if let Some(loc) = obj.get("location").and_then(|v| v.as_str()) {
            if !loc.is_empty() {
                println!("  Location: {}", loc);
            }
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

fn ensure_default_config(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    write_default_config(path)
}

fn write_default_config(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {parent:?}"))?;
    }
    let cfg = AppConfig::default();
    let toml = toml::to_string_pretty(&cfg).context("serializing default config")?;
    let mut header = String::new();
    header.push_str("# h8 configuration\n");
    header.push_str(
        "# Place this file at $XDG_CONFIG_HOME/h8/config.toml (or ~/.config/h8/config.toml)\n\n",
    );
    fs::write(path, format!("{header}{toml}\n"))
        .with_context(|| format!("writing config file to {}", path.display()))
}

fn load_config(paths: &mut AppPaths, common: &CommonOpts) -> Result<AppConfig> {
    let env_prefix = env_prefix();
    let mut builder = Config::builder()
        .add_source(
            File::from(paths.global_config.as_path())
                .format(FileFormat::Toml)
                .required(false),
        )
        .add_source(
            File::from(paths.local_config.as_path())
                .format(FileFormat::Toml)
                .required(false),
        )
        .add_source(Environment::with_prefix(env_prefix.as_str()).separator("__"));

    if let Some(cli_cfg) = &paths.cli_config {
        builder = builder.add_source(
            File::from(cli_cfg.as_path())
                .format(FileFormat::Toml)
                .required(true),
        );
    }

    builder = builder
        .set_default("account", AppConfig::default().account.clone())?
        .set_default("timezone", AppConfig::default().timezone.clone())?
        .set_default("service_url", AppConfig::default().service_url.clone())?
        .set_default("free_slots.start_hour", 9)?
        .set_default("free_slots.end_hour", 17)?
        .set_default("free_slots.exclude_weekends", true)?;

    let mut config: AppConfig = builder.build()?.try_deserialize()?;

    if let Some(account) = &common.account {
        config.account = account.clone();
    }

    Ok(config)
}

fn expand_path(path: PathBuf) -> Result<PathBuf> {
    if let Some(text) = path.to_str() {
        expand_str_path(text)
    } else {
        Ok(path)
    }
}

fn expand_str_path(text: &str) -> Result<PathBuf> {
    let expanded = shellexpand::full(text).context("expanding path")?;
    Ok(PathBuf::from(expanded.to_string()))
}

fn default_config_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        let mut path = PathBuf::from(dir);
        path.push(APP_NAME);
        return Ok(path);
    }
    if let Some(mut dir) = dirs::config_dir() {
        dir.push(APP_NAME);
        return Ok(dir);
    }
    dirs::home_dir()
        .map(|home| home.join(".config").join(APP_NAME))
        .ok_or_else(|| anyhow!("unable to determine configuration directory"))
}

fn default_state_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        let mut path = PathBuf::from(dir);
        path.push(APP_NAME);
        return Ok(path);
    }
    if let Some(mut dir) = dirs::state_dir() {
        dir.push(APP_NAME);
        return Ok(dir);
    }
    dirs::home_dir()
        .map(|home| home.join(".local").join("state").join(APP_NAME))
        .ok_or_else(|| anyhow!("unable to determine state directory"))
}

fn env_prefix() -> String {
    APP_NAME
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn service_pid_path(ctx: &RuntimeContext) -> Result<PathBuf> {
    fs::create_dir_all(&ctx.paths.state_dir)
        .with_context(|| format!("creating state directory {}", ctx.paths.state_dir.display()))?;
    Ok(ctx.paths.state_dir.join("service.pid"))
}

fn read_pid(path: &Path) -> Result<Option<u32>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).context("reading pid file")?;
    let pid: u32 = text.trim().parse().context("parsing pid file")?;
    Ok(Some(pid))
}

fn start_service(ctx: &RuntimeContext) -> Result<()> {
    let pid_path = service_pid_path(ctx)?;
    if let Some(pid) = read_pid(&pid_path)? {
        if pid_running(pid) {
            return Err(anyhow!("service already running with pid {}", pid));
        }
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

#[derive(Debug, Clone)]
struct ServiceClient {
    http: reqwest::blocking::Client,
    base_url: String,
}

impl ServiceClient {
    fn new(ctx: &RuntimeContext) -> Result<Self> {
        let mut builder = reqwest::blocking::Client::builder();
        if let Some(secs) = ctx.common.timeout {
            builder = builder.timeout(Duration::from_secs(secs));
        }
        let http = builder.build().context("building HTTP client")?;
        let base_url = ctx.config.service_url.trim_end_matches('/').to_string();
        Ok(Self { http, base_url })
    }

    fn calendar_list(
        &self,
        account: &str,
        days: i64,
        from_date: Option<&str>,
        to_date: Option<&str>,
    ) -> Result<Value> {
        let mut req = self
            .http
            .get(format!("{}/calendar", self.base_url))
            .query(&[("account", account), ("days", &days.to_string())]);
        if let Some(f) = from_date {
            req = req.query(&[("from_date", f)]);
        }
        if let Some(t) = to_date {
            req = req.query(&[("to_date", t)]);
        }
        Self::send(req)
    }

    fn calendar_create(&self, account: &str, payload: Value) -> Result<Value> {
        let req = self
            .http
            .post(format!("{}/calendar", self.base_url))
            .query(&[("account", account)])
            .json(&payload);
        Self::send(req)
    }

    fn calendar_delete(&self, account: &str, id: &str, change_key: Option<&str>) -> Result<Value> {
        let mut req = self
            .http
            .delete(format!("{}/calendar/{}", self.base_url, id))
            .query(&[("account", account)]);
        if let Some(ck) = change_key {
            req = req.query(&[("changekey", ck)]);
        }
        Self::send(req)
    }

    fn mail_list(&self, account: &str, folder: &str, limit: usize, unread: bool) -> Result<Value> {
        let req = self.http.get(format!("{}/mail", self.base_url)).query(&[
            ("account", account),
            ("folder", folder),
            ("limit", &limit.to_string()),
            ("unread", &unread.to_string()),
        ]);
        Self::send(req)
    }

    fn mail_get(&self, account: &str, folder: &str, id: &str) -> Result<Value> {
        let req = self
            .http
            .get(format!("{}/mail/{}", self.base_url, id))
            .query(&[("account", account), ("folder", folder)]);
        Self::send(req)
    }

    fn mail_send(&self, account: &str, payload: Value) -> Result<Value> {
        let req = self
            .http
            .post(format!("{}/mail/send", self.base_url))
            .query(&[("account", account)])
            .json(&payload);
        Self::send(req)
    }

    fn mail_fetch(
        &self,
        account: &str,
        folder: &str,
        output: &Path,
        format: FetchFormat,
        limit: Option<usize>,
    ) -> Result<Value> {
        let mut body = serde_json::json!({
            "folder": folder,
            "output": output.display().to_string(),
            "format": match format { FetchFormat::Maildir => "maildir", FetchFormat::Mbox => "mbox" },
        });
        if let Some(lim) = limit {
            body["limit"] = serde_json::json!(lim);
        }
        let req = self
            .http
            .post(format!("{}/mail/fetch", self.base_url))
            .query(&[("account", account)])
            .json(&body);
        Self::send(req)
    }

    fn contacts_list(&self, account: &str, limit: usize, search: Option<&str>) -> Result<Value> {
        let mut req = self
            .http
            .get(format!("{}/contacts", self.base_url))
            .query(&[("account", account), ("limit", &limit.to_string())]);
        if let Some(s) = search {
            req = req.query(&[("search", s)]);
        }
        Self::send(req)
    }

    fn contacts_get(&self, account: &str, id: &str) -> Result<Value> {
        let req = self
            .http
            .get(format!("{}/contacts/{}", self.base_url, id))
            .query(&[("account", account)]);
        Self::send(req)
    }

    fn contacts_create(&self, account: &str, payload: Value) -> Result<Value> {
        let req = self
            .http
            .post(format!("{}/contacts", self.base_url))
            .query(&[("account", account)])
            .json(&payload);
        Self::send(req)
    }

    fn contacts_delete(&self, account: &str, id: &str) -> Result<Value> {
        let req = self
            .http
            .delete(format!("{}/contacts/{}", self.base_url, id))
            .query(&[("account", account)]);
        Self::send(req)
    }

    fn free_slots(
        &self,
        account: &str,
        weeks: u8,
        duration: u32,
        limit: Option<usize>,
    ) -> Result<Value> {
        let mut req = self.http.get(format!("{}/free", self.base_url)).query(&[
            ("account", account),
            ("weeks", &weeks.to_string()),
            ("duration", &duration.to_string()),
        ]);
        if let Some(lim) = limit {
            req = req.query(&[("limit", &lim.to_string())]);
        }
        Self::send(req)
    }

    fn send(req: reqwest::blocking::RequestBuilder) -> Result<Value> {
        let resp = req.send().context("sending request to service")?;
        let status = resp.status();
        let text = resp.text().context("reading service response")?;
        if !status.is_success() {
            if let Ok(val) = serde_json::from_str::<Value>(&text) {
                if let Some(detail) = val
                    .as_object()
                    .and_then(|m| m.get("detail"))
                    .and_then(|d| d.as_str())
                {
                    return Err(anyhow!("service error: {}", detail));
                }
            }
            let snippet: String = text.chars().take(400).collect();
            return Err(anyhow!("service error: {}", snippet));
        }
        serde_json::from_str(&text).context("parsing JSON from service")
    }
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
        let end_tick = ((slot.end_min + minutes_per_tick - 1) / minutes_per_tick)
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
