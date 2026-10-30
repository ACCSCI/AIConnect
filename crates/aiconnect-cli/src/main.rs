mod ipc;

mod runtime;

mod update;

#[cfg(windows)]

mod windows_process;



use anyhow::{Context, Result, bail};

use clap::{Parser, Subcommand};

use serde_json::{Value, json};

use std::{path::PathBuf, time::Duration};



#[derive(Parser)]

#[command(

    name = "aiconnect",

    version,

    about = "Local-first encrypted Agent communication"

)]

struct Cli {

    #[arg(long, global = true, env = "AICONNECT_HOME")]

    data: Option<PathBuf>,

    #[arg(long, global = true)]

    json: bool,

    #[command(subcommand)]

    command: Command,

}



#[derive(Subcommand)]

enum Command {

    /// Install a signed native update, or check/configure/roll back releases.

    Update {

        #[command(subcommand)]

        command: Option<update::Action>,

    },

    Setup {

        #[arg(long)]

        name: Option<String>,

        #[arg(long, conflicts_with = "relay_only")]

        direct_only: bool,

        #[arg(long)]

        relay_only: bool,

        /// Opt in to n0 public relays for development/testing, without a production guarantee.

        #[arg(long, conflicts_with = "direct_only")]

        dev_public_relay: bool,

    },

    Daemon {

        #[command(subcommand)]

        command: DaemonCommand,

    },

    Status,

    Doctor {

        /// Inspect actual local transport configuration and observed paths without contacting services.

        #[arg(long)]

        network: bool,

        /// Explicitly probe this existing contact; does not create a session or send chat.

        #[arg(long, requires = "network")]

        peer: Option<String>,

    },

    #[command(alias = "id", alias = "card")]

    Identity {

        #[arg(long)]

        name: Option<String>,

    },

    Contacts {

        #[command(subcommand)]

        command: ContactCommand,

    },

    Temp {

        #[command(subcommand)]

        command: TempCommand,

    },

    Connect {

        id: String,

    },

    Send {

        session: String,

        #[arg(long)]

        text: String,

        #[arg(long, default_value="text", value_parser=["text", "task"])]

        kind: String,

        #[arg(long)]

        id: Option<String>,

    },

    Messages {

        session: String,

        #[arg(long, default_value_t = 0)]

        after: u64,

    },

    Sessions,

    Session {

        #[command(subcommand)]

        command: SessionCommand,

    },

    Inbox {

        #[arg(long, default_value_t = 0)]

        after: u64,

        #[arg(long, alias = "watch")]

        watch: bool,

    },

    Retry,

    /// Explicit task proposal, acceptance, delivery and requester review.

    Task {

        #[command(subcommand)]

        command: TaskCommand,

    },

    /// Multi-agent group messaging with owner-controlled membership.

    Group {

        #[command(subcommand)]

        command: GroupCommand,

    },

    /// Send a structured local core request (diagnostics and automation).

    Request {

        json_request: String,

    },

    Agent {

        #[command(subcommand)]

        command: runtime::AgentCommand,

    },

    /// Account root and scoped device delegations (separate from group ownership).

    Owner {

        #[command(subcommand)]

        command: OwnerCommand,

    },

    Profile {

        #[command(subcommand)]

        command: ProfileCommand,

    },

    Capability {

        #[command(subcommand)]

        command: CapabilityCommand,

    },

    /// Search verified local discovery caches; does not authorize execution.

    Discover(DiscoverArgs),

    Directory {

        #[command(subcommand)]

        command: DirectoryCommand,

    },

    Discovery {

        #[command(subcommand)]

        command: DiscoveryCommand,

    },

    Channel {

        #[command(subcommand)]

        command: ChannelCommand,

    },

    Service {

        #[command(subcommand)]

        command: ServiceCommand,

    },

    Policy {

        #[command(subcommand)]

        command: PolicyCommand,

    },

    Budget {

        #[command(subcommand)]

        command: BudgetCommand,

    },

    Audit {

        #[command(subcommand)]

        command: AuditCommand,

    },

    Provider {

        #[command(subcommand)]

        command: ProviderCommand,

    },

    /// Review unknown requests without waking a runtime.

    Requests {

        #[command(subcommand)]

        command: RequestsCommand,

    },

    /// Optional local protocol adapters; never installs dependencies automatically.

    Bridge {

        #[command(subcommand)]

        command: BridgeCommand,

    },

}



#[derive(Subcommand)]

enum DaemonCommand {

    Start {

        #[arg(long, conflicts_with = "relay_only")]

        direct_only: bool,

        #[arg(long)]

        relay_only: bool,

        #[arg(long)]

        relay: Option<String>,

        #[arg(long, conflicts_with = "direct_only")]

        dev_public_relay: bool,

    },

    Stop,

    #[command(hide = true)]

    Run {

        #[arg(long, conflicts_with = "relay_only")]

        direct_only: bool,

        #[arg(long)]

        relay_only: bool,

        #[arg(long)]

        relay: Option<String>,

        #[arg(long, hide = true)]

        background: bool,

        #[arg(long, conflicts_with = "direct_only")]

        dev_public_relay: bool,

    },

}



#[derive(Subcommand)]

enum ContactCommand {

    List,

    /// Send a contact request to the card's endpoint without importing or authorizing it.

    Request {

        card: PathBuf,

    },

    Import {

        card: PathBuf,

        #[arg(long)]

        name: Option<String>,

        #[arg(long)]

        group: Option<String>,

    },

    Export {

        id: String,

        #[arg(long)]

        output: Option<PathBuf>,

    },

    Remove {

        id: String,

    },

    Block {

        id: String,

    },

    Unblock {

        id: String,

    },

    Group {

        id: String,

        group: String,

    },

    Policy {

        id: String,

        #[arg(long, action=clap::ArgAction::Set)]

        chat: Option<bool>,

        #[arg(long, action=clap::ArgAction::Set)]

        wake: Option<bool>,

        #[arg(long, action=clap::ArgAction::Set)]

        task: Option<bool>,

    },

}



#[derive(Subcommand)]

enum TempCommand {

    Create {

        #[arg(long, default_value = "Temporary collaboration")]

        name: String,

        #[arg(long, default_value="30m", value_parser=parse_ttl)]

        ttl: u64,

    },

    Join {

        invite: String,

    },

}

#[derive(Subcommand)]

enum SessionCommand {

    Close { id: String },

}



#[derive(Subcommand)]

enum GroupCommand {

    /// Change an existing group's signed capacity (owner only).

    Limit {

        group: String,

        #[arg(long, value_parser=clap::value_parser!(u64).range(2..=64))]

        max_members: u64,

    },

    Create {

        #[arg(long)]

        name: String,

        #[arg(long,value_parser=clap::value_parser!(u64).range(2..=64))]

        max_members: Option<u64>,

        /// Retain the legacy protocol that requires the owner online for every message.

        #[arg(long)]

        owner_online: bool,

    },

    List {

        #[arg(long, default_value_t = 0)]

        offset: u64,

    },

    Show {

        group: String,

    },

    Sync {

        group: String,

    },

    /// Enable member replication for an existing group (owner only).

    Replicate {

        group: String,

    },

    Invite {

        group: String,

        peer: String,

        #[arg(long,default_value="15m",value_parser=parse_ttl)]

        ttl: u64,

    },

    Join {

        invite: String,

    },

    Remove {

        group: String,

        peer: String,

    },

    Role {

        group: String,

        peer: String,

        #[arg(value_parser=["admin","member"])]

        role: String,

    },

    Policy {

        group: String,

        peer: String,

        #[arg(long,required=true,action=clap::ArgAction::Set)]

        can_send: bool,

    },

    Leave {

        group: String,

    },

    Dissolve {

        group: String,

    },

    Send {

        group: String,

        #[arg(long)]

        text: String,

        #[arg(long)]

        id: Option<String>,

    },

    Messages {

        group: String,

        #[arg(long, default_value_t = 0)]

        after: u64,

    },

    Inbox {

        #[arg(long, default_value_t = 0)]

        after: u64,

        #[arg(long)]

        watch: bool,

    },

}



#[derive(Subcommand)]

enum TaskCommand {

    Propose {

        session: String,

        #[arg(long)]

        text: String,

        #[arg(long, default_value = "Task")]

        title: String,

        #[arg(long, default_value = "1h", value_parser=parse_ttl)]

        ttl: u64,

        #[arg(long)]

        contract_id: Option<String>,

        #[arg(long)]

        event_id: Option<String>,

    },

    List {

        #[arg(long, default_value_t = 0)]

        offset: u64,

    },

    Show {

        task: String,

    },

    History {

        task: String,

        #[arg(long, default_value_t = 0)]

        after: u64,

    },

    Accept {

        task: String,

        #[arg(long)]

        event_id: Option<String>,

    },

    Decline {

        task: String,

        #[arg(long)]

        reason: String,

        #[arg(long)]

        event_id: Option<String>,

    },

    Deliver {

        task: String,

        #[arg(long)]

        text: String,

        #[arg(long)]

        sha256: Option<String>,

        #[arg(long)]

        event_id: Option<String>,

    },

    Review {

        task: String,

        #[arg(long, required_unless_present = "reject", conflicts_with = "reject")]

        approve: bool,

        #[arg(long)]

        reject: bool,

        #[arg(long)]

        note: String,

        #[arg(long)]

        event_id: Option<String>,

    },

}



#[derive(Subcommand)]

enum OwnerCommand {

    Init,

    Inspect,

    Rotate,

    /// Export only an encrypted root backup; Windows DPAPI is not portable across machines.

    Backup {

        #[arg(long)]

        output: Option<PathBuf>,

    },

    /// Restore a pinned encrypted root without overwriting an existing root.

    Restore {

        file: PathBuf,

        #[arg(long)]

        expected_owner: String,

    },

    /// Issue a short-lived delegation. Root secrets remain on this device.

    Grant(GrantArgs),

    PairRequest {

        #[arg(long)]

        output: Option<PathBuf>,

    },

    Pair {

        file: PathBuf,

        #[arg(long)]

        audience: String,

        #[arg(long, required = true, value_delimiter = ',')]

        scopes: Vec<String>,

        #[arg(long,default_value="1h",value_parser=parse_ttl)]

        ttl: u64,

        #[arg(long, default_value_t = 1)]

        max_calls: u64,

        #[arg(long)]

        output: Option<PathBuf>,

    },

    RevocationExport {

        #[arg(long)]

        output: Option<PathBuf>,

    },

    RotationImport {

        file: PathBuf,

    },

    DomainChallenge {

        domain: String,

        #[arg(long)]

        output: Option<PathBuf>,

    },

    DomainVerify {

        domain: String,

        #[arg(long)]

        expected_owner: String,

    },

    DomainInspect {

        domain: String,

    },

    /// Import a grant with an explicitly pinned owner key.

    Import {

        file: PathBuf,

        #[arg(long)]

        expected_owner: String,

        #[arg(long, required = true)]

        trust_owner: bool,

    },

    Verify {

        grant_id: String,

        #[arg(long)]

        audience: String,

        #[arg(long)]

        scope: String,

    },

    Revoke {

        grant_id: String,

    },

    Revocations {

        file: PathBuf,

    },

}

#[derive(clap::Args)]

struct GrantArgs {

    agent_id: String,

    #[arg(long)]

    audience: String,

    #[arg(long, required = true, value_delimiter = ',')]

    scopes: Vec<String>,

    #[arg(long, default_value="1h",value_parser=parse_ttl)]

    ttl: u64,

    #[arg(long, default_value_t = 1)]

    max_calls: u64,

    #[arg(long)]

    output: Option<PathBuf>,

}

#[derive(Subcommand)]

enum ProfileCommand {

    Set {

        #[arg(long, value_parser=["private","lan","public"],default_value="private")]

        visibility: String,

    },

}

#[derive(Subcommand)]

enum CapabilityCommand {

    /// Sign a local descriptor; does not upload it or execute its skills.

    Publish {

        file: PathBuf,

        #[arg(long)]

        output: Option<PathBuf>,

    },

    Inspect,

    Validate {

        file: Option<PathBuf>,

        #[arg(long)]

        skill_id: Option<String>,

        #[arg(long, requires = "skill_id")]

        input: Option<PathBuf>,

    },

    DryRun {

        file: Option<PathBuf>,

        #[arg(long)]

        skill_id: Option<String>,

        #[arg(long, requires = "skill_id")]

        input: Option<PathBuf>,

    },

}

#[derive(clap::Args)]

struct DiscoverArgs {

    #[arg(long, value_parser=["mine","trusted","public","all"],default_value="all")]

    scope: String,

    #[arg(long)]

    query: Option<String>,

    #[arg(long)]

    capability: Option<String>,

    #[arg(long)]

    language: Option<String>,

    #[arg(long)]

    os: Option<String>,

    #[arg(long)]

    include_expired: bool,

    #[arg(long, default_value_t = 50)]

    limit: u64,

}

#[derive(Subcommand)]

enum DirectoryCommand {

    /// Register a source only after checking its free-use terms.

    Add {

        url: String,

        #[arg(long)]

        id: Option<String>,

        #[arg(long)]

        terms_url: String,

        #[arg(long, required = true)]

        free: bool,

        #[arg(long, required = true)]

        terms_allowed: bool,

    },

    List,

    Remove {

        id: String,

    },

    Sync {

        id: Option<String>,

    },

    Import {

        file: PathBuf,

        #[arg(long)]

        source_id: Option<String>,

        #[arg(long)]

        trusted: bool,

    },

}

#[derive(Subcommand)]

enum DiscoveryCommand {

    Lan {

        #[arg(value_parser=["status","advertise","browse","stop"])]

        mode: String,

        #[arg(long, default_value_t = 3)]

        seconds: u64,

    },

    /// Nostr announcement sources, not iroh transport relays.

    Relay {

        #[command(subcommand)]

        command: DiscoveryRelayCommand,

    },

    /// Publish/import explicit public announcements; never private metadata.

    Nostr {

        #[command(subcommand)]

        command: NostrCommand,

    },

}

#[derive(Subcommand)]

enum NostrCommand {

    Export {

        #[arg(long, required = true)]

        confirm_public: bool,

        #[arg(long)]

        output: Option<PathBuf>,

    },

    Import {

        file: PathBuf,

        #[arg(long)]

        source_id: Option<String>,

    },

    Publish {

        id: String,

        #[arg(long, required = true)]

        confirm_public: bool,

    },

    Read {

        id: String,

        #[arg(long, default_value_t = 20)]

        limit: u64,

    },

}

#[derive(Subcommand)]

enum DiscoveryRelayCommand {

    Add {

        url: String,

        #[arg(long)]

        terms_url: String,

        #[arg(long, required = true)]

        free: bool,

        #[arg(long, required = true)]

        terms_allowed: bool,

    },

    List,

    Remove {

        id: String,

    },

    Probe {

        id: String,

    },

}

#[derive(Subcommand)]

enum ChannelCommand {

    /// Stream bounded notification JSONL; never executes or wakes an Agent.

    Watch {

        #[arg(long, default_value_t = 0)]

        after: u64,

    },

    Create {

        #[arg(long)]

        name: String,

        #[arg(long,default_value="private",value_parser=["private","public"])]

        visibility: String,

        #[arg(long)]

        max_members: Option<u64>,

    },

    List,

    Show {

        channel: String,

    },

    Invite {

        channel: String,

        peer: String,

    },

    Join {

        invite: String,

    },

    Publisher {

        channel: String,

        peer: String,

        #[arg(long,action=clap::ArgAction::Set)]

        can_publish: bool,

    },

    Publish {

        channel: String,

        #[arg(long)]

        text: String,

        #[arg(long)]

        id: Option<String>,

    },

    Read {

        #[arg(long, conflicts_with = "source")]

        channel: Option<String>,

        #[arg(long, required_unless_present = "channel")]

        source: Option<String>,

        #[arg(long, default_value_t = 0)]

        after: u64,

    },

    /// Write signed static files locally; never deploys them.

    Export {

        channel: String,

        #[arg(long)]

        output: PathBuf,

    },

    Subscribe {

        #[arg(long, conflicts_with = "channel")]

        url: Option<String>,

        #[arg(long, required_unless_present = "url")]

        channel: Option<String>,

        #[arg(long)]

        source_id: Option<String>,

        #[arg(long)]

        expected_publisher: Option<String>,

        /// Alternative HTTPS copies of the same publisher-signed channel (up to 3).

        #[arg(long = "mirror", requires = "expected_publisher", requires = "url")]

        mirrors: Vec<String>,

        /// Explicitly authorize read-only local runtime notifications; never channel replies.

        #[arg(long)]

        wake: bool,

    },

    Wake {

        source: String,

        #[arg(long,action=clap::ArgAction::Set)]

        enabled: bool,

    },

    Subscriptions,

    Sync {

        source: String,

    },

    Import {

        source: String,

        file: PathBuf,

    },

    Unsubscribe {

        source: String,

    },

    Mute {

        source: String,

        #[arg(long,default_value="true",action=clap::ArgAction::Set)]

        muted: bool,

    },

}

#[derive(Subcommand)]

enum ServiceCommand {

    Inspect {

        #[arg(long)]

        provider: Option<String>,

        #[arg(long)]

        capability: Option<String>,

    },

    Quote {

        file: PathBuf,

    },

    /// Invoke from a JSON request containing grant, zero-price quote, and idempotency key.

    Invoke {

        file: PathBuf,

    },

    Status {

        job: String,

    },

    Result {

        job: String,

    },

    Cancel {

        job: String,

    },

    Review {

        job: String,

        #[arg(long, required_unless_present = "reject", conflicts_with = "reject")]

        approve: bool,

        #[arg(long)]

        reject: bool,

        #[arg(long)]

        text: Option<String>,

    },

    List {

        #[arg(long, default_value_t = 0)]

        offset: u64,

    },

}

#[derive(Subcommand)]

enum PolicyCommand {

    Inspect,

    Set {

        file: PathBuf,

    },

    Import {

        file: PathBuf,

        #[arg(long)]

        label: Option<String>,

    },

    Adopt {

        proposal_id: String,

    },

}

#[derive(Subcommand)]

enum BudgetCommand {

    Inspect,

    Reserve { file: PathBuf },

    Commit { reservation_id: String },

    Release { reservation_id: String },

}

#[derive(Subcommand)]

enum AuditCommand {

    Export {

        #[arg(long, default_value_t = 0)]

        after: u64,

        #[arg(long, default_value_t = 1000)]

        limit: u64,

        #[arg(long)]

        output: Option<PathBuf>,

    },

}

#[derive(Subcommand)]

enum ProviderCommand {

    Add {

        url: String,

        #[arg(long)]

        terms_url: String,

        #[arg(long)]

        purpose: String,

        #[arg(long, required = true)]

        free: bool,

        #[arg(long, required = true)]

        terms_allowed: bool,

    },

    List,

    Check {

        url: String,

    },

}

#[derive(Subcommand)]

enum RequestsCommand {

    List,

    Decide {

        id: String,

        #[arg(value_parser=["allow","deny"])]

        decision: String,

    },

}

#[derive(Subcommand)]

enum BridgeCommand {

    /// Run an installed optional adapter in the foreground (Ctrl-C to stop).

    A2a {

        #[command(subcommand)]

        command: BridgeAdapterCommand,

    },

    Mcp {

        #[command(subcommand)]

        command: BridgeAdapterCommand,

    },

}

#[derive(Subcommand)]

enum BridgeAdapterCommand {

    /// Inspect a free authorized MCP Registry URL without running the listed servers (MCP only).

    Registry {

        #[arg(long)]

        url: String,

        #[arg(long)]

        script: Option<PathBuf>,

    },

    Start {

        #[arg(long)]

        script: Option<PathBuf>,

        #[arg(long)]

        once: bool,

        #[arg(long)]

        config: Option<PathBuf>,

    },

    /// Inspect installation; does not infer a running daemon from a stale PID.

    Status {

        #[arg(long)]

        script: Option<PathBuf>,

    },

    /// Explicitly serve the configured Agent on loopback HTTP (A2A only).

    Serve {

        #[arg(long)]

        config: PathBuf,

        #[arg(long)]

        script: Option<PathBuf>,

    },

    /// Inspect an external Agent Card through the local policy boundary (A2A only).

    Inspect {

        #[arg(long)]

        url: String,

        #[arg(long)]

        script: Option<PathBuf>,

        #[arg(long)]

        config: Option<PathBuf>,

    },

}



fn parse_ttl(value: &str) -> std::result::Result<u64, String> {

    let (number, multiplier) = if let Some(v) = value.strip_suffix('m') {

        (v, 60)

    } else if let Some(v) = value.strip_suffix('h') {

        (v, 3600)

    } else if let Some(v) = value.strip_suffix('s') {

        (v, 1)

    } else {

        (value, 1)

    };

    let seconds = number

        .parse::<u64>()

        .ok()

        .and_then(|n| n.checked_mul(multiplier))

        .ok_or("TTL must be seconds or a duration such as 30m or 1h")?;

    if !(1..=86400).contains(&seconds) {

        return Err("TTL must be between 1 second and 24 hours".into());

    }

    Ok(seconds)

}



fn emit(value: &Value, compact: bool) -> Result<()> {

    println!(

        "{}",

        if compact {

            serde_json::to_string(value)?

        } else {

            serde_json::to_string_pretty(value)?

        }

    );

    Ok(())

}



#[tokio::main]

async fn main() {

    let cli = Cli::parse();

    if let Err(error) = execute(cli).await {

        eprintln!("{}", json!({"error":error.to_string()}));

        std::process::exit(1);

    }

}



async fn execute(cli: Cli) -> Result<()> {

    let directory = cli.data.unwrap_or_else(|| {

        dirs::home_dir()

            .unwrap_or_else(|| PathBuf::from("."))

            .join(".aiconnect")

    });

    let directory = std::path::absolute(directory)?;

    let mut output_file: Option<PathBuf> = None;

    let request = match cli.command {

        Command::Update { command } => {

            return emit(&update::run(&directory, command).await?, cli.json);

        }

        Command::Daemon {

            command:

                DaemonCommand::Run {

                    direct_only,

                    relay_only,

                    relay,

                    background,

                    dev_public_relay,

                },

        } => {

            let update_directory = directory.clone();

            let update_check =

                tokio::spawn(async move { update::startup(&update_directory).await });

            let result = ipc::run(

                directory.clone(),

                transport_direct(direct_only, relay_only, relay.as_deref(), dev_public_relay)?,

                relay_only,

                relay,

                background,

            )

            .await;

            update_check.abort();

            if let Err(error) = &result {

                use std::io::Write;

                if let Ok(mut log) = std::fs::OpenOptions::new()

                    .create(true)

                    .append(true)

                    .open(directory.join("daemon.log"))

                {

                    let _ = writeln!(log, "daemon_error: {error}");

                }

            }

            return result;

        }

        Command::Daemon {

            command:

                DaemonCommand::Start {

                    direct_only,

                    relay_only,

                    relay,

                    dev_public_relay,

                },

        } => {

            return emit(

                &ipc::start(

                    &directory,

                    transport_direct(direct_only, relay_only, relay.as_deref(), dev_public_relay)?,

                    relay_only,

                    relay,

                )

                .await?,

                cli.json,

            );

        }

        Command::Daemon {

            command: DaemonCommand::Stop,

        } => return emit(&ipc::stop(&directory).await?, cli.json),

        Command::Setup {

            name,

            direct_only,

            relay_only,

            dev_public_relay,

        } => {

            ipc::start(

                &directory,

                transport_direct(direct_only, relay_only, None, dev_public_relay)?,

                relay_only,

                None,

            )

            .await?;

            let identity = ipc::call(&directory, json!({"action":"identity","name":name})).await?;

            return emit(

                &json!({"ready":true,"dataDirectory":directory,"identity":identity,"status":ipc::call(&directory,json!({"action":"status"})).await?}),

                cli.json,

            );

        }

        Command::Status => json!({"action":"status"}),

        Command::Doctor { network, peer } => {

            let status = ipc::call(&directory, json!({"action":"status"})).await;

            return match status {

                Ok(status) => {

                    let mut report = json!({"ok":true,"dataDirectory":directory,"ipc":"authenticated_loopback_json","status":status,"checks":{"daemon":true,"localIpc":true},"networkReachability":"not_proven_by_status"});

                    if network {

                        let diagnostics =

                            ipc::call(&directory, json!({"action":"network.diagnostics"})).await;

                        report["network"] = match diagnostics {

                            Ok(v) => v,

                            Err(e) => {

                                json!({"available":false,"error":e.to_string(),"observedLastPath":report["status"]["lastPath"],"wanProven":false})

                            }

                        };

                        let providers =

                            ipc::call(&directory, json!({"action":"provider.list"})).await;

                        report["freeProviders"] = match providers {

                            Ok(v) => v,

                            Err(e) => json!({"available":false,"error":e.to_string()}),

                        };

                        if let Some(peer) = peer {

                            report["peerProbe"] = match ipc::call(

                                &directory,

                                json!({"action":"network.probe","peer":peer}),

                            )

                            .await

                            {

                                Ok(v) => v,

                                Err(e) => {

                                    json!({"reachable":false,"error":e.to_string(),"globalWanProven":false})

                                }

                            };

                        }

                    }

                    emit(&report, cli.json)

                }

                Err(error) => {

                    emit(

                        &json!({"ok":false,"dataDirectory":directory,"checks":{"daemon":false},"error":error.to_string(),"remedy":"Run aiconnect setup or daemon start"}),

                        cli.json,

                    )?;

                    bail!("doctor_failed")

                }

            };

        }

        Command::Identity { name } => json!({"action":"identity","name":name}),

        Command::Contacts { command } => match command {

            ContactCommand::List => json!({"action":"contacts.list"}),

            ContactCommand::Request { card } => {

                json!({"action":"contacts.request","card":read_json(&card)?})

            }

            ContactCommand::Import { card, name, group } => {

                json!({"action":"contacts.import","card":serde_json::from_slice::<Value>(&std::fs::read(&card).with_context(||format!("Cannot read card {}",card.display()))?)?,"name":name,"group":group})

            }

            ContactCommand::Export { id, output } => {

                let card =

                    ipc::call(&directory, json!({"action":"contacts.export","id":id})).await?;

                if let Some(path) = output {

                    std::fs::write(&path, serde_json::to_vec_pretty(&card)?)?;

                    return emit(&json!({"written":path}), cli.json);

                }

                return emit(&card, cli.json);

            }

            ContactCommand::Remove { id } => json!({"action":"contacts.remove","id":id}),

            ContactCommand::Block { id } => {

                json!({"action":"contacts.block","id":id,"blocked":true})

            }

            ContactCommand::Unblock { id } => {

                json!({"action":"contacts.block","id":id,"blocked":false})

            }

            ContactCommand::Group { id, group } => {

                json!({"action":"contacts.group","id":id,"group":group})

            }

            ContactCommand::Policy {

                id,

                chat,

                wake,

                task,

            } => {

                if chat.is_none() && wake.is_none() && task.is_none() {

                    bail!("Specify at least one of --chat, --wake, --task")

                }

                let mut value = json!({"action":"contacts.policy","id":id});

                for (key, val) in [("chat", chat), ("wake", wake), ("task", task)] {

                    if let Some(v) = val {

                        value[key] = json!(v)

                    }

                }

                value

            }

        },

        Command::Temp {

            command: TempCommand::Create { name, ttl },

        } => json!({"action":"temp.create","label":name,"ttlSeconds":ttl}),

        Command::Temp {

            command: TempCommand::Join { invite },

        } => json!({"action":"temp.join","invite":invite}),

        Command::Connect { id } => json!({"action":"connect","id":id}),

        Command::Send {

            session,

            text,

            kind,

            id,

        } => json!({"action":"send","sessionId":session,"text":text,"kind":kind,"id":id}),

        Command::Messages { session, after } => {

            json!({"action":"messages","sessionId":session,"after":after})

        }

        Command::Sessions => json!({"action":"sessions"}),

        Command::Session {

            command: SessionCommand::Close { id },

        } => json!({"action":"session.close","sessionId":id}),

        Command::Inbox { after, watch } => {

            if !watch {

                json!({"action":"inbox","after":after})

            } else {

                let mut cursor = after;

                loop {

                    let value =

                        ipc::call(&directory, json!({"action":"inbox","after":cursor})).await?;

                    if let Some(messages) = value["messages"].as_array() {

                        for message in messages {

                            cursor = cursor.max(message["sequence"].as_u64().unwrap_or(cursor));

                            emit(message, true)?;

                        }

                    }

                    tokio::select! {_=tokio::time::sleep(Duration::from_millis(350))=>{},_=tokio::signal::ctrl_c()=>return Ok(())}

                }

            }

        }

        Command::Retry => json!({"action":"retry"}),

        Command::Group { command } => match command {

            GroupCommand::Limit { group, max_members } => {

                json!({"action":"groups.limit","groupId":group,"maxMembers":max_members})

            }

            GroupCommand::Create {

                name,

                max_members,

                owner_online,

            } => {

                json!({"action":"groups.create","name":name,"maxMembers":max_members,"ownerOnline":owner_online})

            }

            GroupCommand::List { offset } => json!({"action":"groups.list","offset":offset}),

            GroupCommand::Show { group } => json!({"action":"groups.show","groupId":group}),

            GroupCommand::Sync { group } => json!({"action":"groups.sync","groupId":group}),

            GroupCommand::Replicate { group } => {

                json!({"action":"groups.replicate","groupId":group})

            }

            GroupCommand::Invite { group, peer, ttl } => {

                json!({"action":"groups.invite","groupId":group,"peer":peer,"ttlSeconds":ttl})

            }

            GroupCommand::Join { invite } => json!({"action":"groups.join","invite":invite}),

            GroupCommand::Remove { group, peer } => {

                json!({"action":"groups.remove","groupId":group,"peer":peer})

            }

            GroupCommand::Role { group, peer, role } => {

                json!({"action":"groups.role","groupId":group,"peer":peer,"role":role})

            }

            GroupCommand::Policy {

                group,

                peer,

                can_send,

            } => json!({"action":"groups.policy","groupId":group,"peer":peer,"canSend":can_send}),

            GroupCommand::Leave { group } => json!({"action":"groups.leave","groupId":group}),

            GroupCommand::Dissolve { group } => json!({"action":"groups.dissolve","groupId":group}),

            GroupCommand::Send { group, text, id } => {

                json!({"action":"groups.send","groupId":group,"text":text,"id":id})

            }

            GroupCommand::Messages { group, after } => {

                json!({"action":"groups.messages","groupId":group,"after":after})

            }

            GroupCommand::Inbox { after, watch } => {

                if !watch {

                    json!({"action":"groups.inbox","after":after})

                } else {

                    let mut cursor = after;

                    loop {

                        let value =

                            ipc::call(&directory, json!({"action":"groups.inbox","after":cursor}))

                                .await?;

                        if let Some(messages) = value["messages"].as_array() {

                            for message in messages {

                                cursor = cursor.max(message["sequence"].as_u64().unwrap_or(cursor));

                                emit(message, true)?;

                            }

                        }

                        tokio::select! {_=tokio::time::sleep(Duration::from_millis(350))=>{},_=tokio::signal::ctrl_c()=>return Ok(())}

                    }

                }

            }

        },

        Command::Task { command } => match command {

            TaskCommand::Propose {

                session,

                text,

                title,

                ttl,

                contract_id,

                event_id,

            } => {

                json!({"action":"tasks.propose","sessionId":session,"text":text,"title":title,"ttlSeconds":ttl,"contractId":contract_id,"eventId":event_id})

            }

            TaskCommand::List { offset } => json!({"action":"tasks.list","offset":offset}),

            TaskCommand::Show { task } => json!({"action":"tasks.show","taskId":task}),

            TaskCommand::History { task, after } => {

                json!({"action":"tasks.history","taskId":task,"after":after})

            }

            TaskCommand::Accept { task, event_id } => {

                json!({"action":"tasks.accept","taskId":task,"eventId":event_id})

            }

            TaskCommand::Decline {

                task,

                reason,

                event_id,

            } => json!({"action":"tasks.decline","taskId":task,"text":reason,"eventId":event_id}),

            TaskCommand::Deliver {

                task,

                text,

                sha256,

                event_id,

            } => {

                json!({"action":"tasks.deliver","taskId":task,"text":text,"sha256":sha256,"eventId":event_id})

            }

            TaskCommand::Review {

                task,

                approve,

                note,

                event_id,

                ..

            } => {

                json!({"action":if approve {"tasks.approve"}else{"tasks.reject"},"taskId":task,"text":note,"eventId":event_id})

            }

        },

        Command::Request { json_request } => {

            serde_json::from_str(&json_request).context("Invalid JSON request")?

        }

        Command::Agent { command } => {

            return emit(&runtime::configure(&directory, command).await?, cli.json);

        }

        Command::Owner { command } => owner_request(command, &mut output_file)?,

        Command::Profile {

            command: ProfileCommand::Set { visibility },

        } => json!({"action":"profile.set","visibility":visibility}),

        Command::Capability { command } => match command {

            CapabilityCommand::Inspect => json!({"action":"capability.inspect"}),

            CapabilityCommand::Publish { file, output } => {

                output_file = output;

                file_request("capability.publish", &file)?

            }

            CapabilityCommand::Validate {

                file,

                skill_id,

                input,

            } => descriptor_request("capability.validate", file, skill_id, input)?,

            CapabilityCommand::DryRun {

                file,

                skill_id,

                input,

            } => descriptor_request("capability.dry-run", file, skill_id, input)?,

        },

        Command::Discover(a) => {

            json!({"action":"discover","scope":a.scope,"query":a.query,"capability":a.capability,"language":a.language,"os":a.os,"includeExpired":a.include_expired,"limit":a.limit})

        }

        Command::Directory { command } => directory_request(command)?,

        Command::Discovery {

            command:

                DiscoveryCommand::Nostr {

                    command:

                        NostrCommand::Export {

                            confirm_public,

                            output,

                        },

                },

        } => {

            output_file = output;

            json!({"action":"discovery.nostr.export","confirmPublic":confirm_public})

        }

        Command::Discovery { command } => discovery_request(command)?,

        Command::Channel {

            command: ChannelCommand::Watch { after },

        } => {

            let mut cursor = after;

            loop {

                let value = ipc::call(

                    &directory,

                    json!({"action":"channels.notifications","after":cursor}),

                )

                .await?;

                if let Some(rows) = value["notifications"].as_array() {

                    for row in rows {

                        emit(row, true)?;

                        cursor = cursor.max(row["cursor"].as_u64().unwrap_or(cursor));

                    }

                }

                cursor = cursor.max(value["cursor"].as_u64().unwrap_or(cursor));

                tokio::select! { _=tokio::time::sleep(Duration::from_secs(1))=>{},_=tokio::signal::ctrl_c()=>return Ok(()) }

            }

        }

        Command::Channel {

            command: ChannelCommand::Export { channel, output },

        } => {

            let result = ipc::call(

                &directory,

                json!({"action":"channels.export","channelId":channel}),

            )

            .await?;

            std::fs::create_dir_all(&output)?;

            let files = result["files"]

                .as_object()

                .context("invalid_channel_export")?;

            for (name, body) in files {

                if !matches!(name.as_str(), "channel.json" | "feed.json") {

                    bail!("unsafe_export_filename")

                }

                let path = output.join(name);

                // Never silently replace a user's existing export.

                use std::io::Write;

                let mut f = std::fs::OpenOptions::new()

                    .write(true)

                    .create_new(true)

                    .open(&path)

                    .with_context(|| format!("Cannot create {}", path.display()))?;

                f.write_all(body.as_str().context("invalid_export_body")?.as_bytes())?;

            }

            return emit(

                &json!({"written":output,"files":files.keys().collect::<Vec<_>>(),"deployed":false}),

                cli.json,

            );

        }

        Command::Channel { command } => channel_request(command)?,

        Command::Service { command } => service_request(command)?,

        Command::Policy { command } => match command {

            PolicyCommand::Inspect => json!({"action":"policy.inspect"}),

            PolicyCommand::Set { file } => {

                json!({"action":"policy.set","policy":read_json(&file)?})

            }

            PolicyCommand::Import { file, label } => {

                json!({"action":"policy.import","policy":read_json(&file)?,"label":label})

            }

            PolicyCommand::Adopt { proposal_id } => {

                json!({"action":"policy.adopt","proposalId":proposal_id})

            }

        },

        Command::Budget { command } => match command {

            BudgetCommand::Inspect => json!({"action":"budget.inspect"}),

            BudgetCommand::Reserve { file } => file_request("budget.reserve", &file)?,

            BudgetCommand::Commit { reservation_id } => {

                json!({"action":"budget.commit","reservationId":reservation_id})

            }

            BudgetCommand::Release { reservation_id } => {

                json!({"action":"budget.release","reservationId":reservation_id})

            }

        },

        Command::Audit {

            command:

                AuditCommand::Export {

                    after,

                    limit,

                    output,

                },

        } => {

            output_file = output;

            json!({"action":"audit.export","after":after,"limit":limit})

        }

        Command::Provider { command } => match command {

            ProviderCommand::List => json!({"action":"provider.list"}),

            ProviderCommand::Check { url } => json!({"action":"provider.check","url":url}),

            ProviderCommand::Add {

                url,

                terms_url,

                purpose,

                free,

                terms_allowed,

            } => {

                json!({"action":"provider.add","url":url,"termsUrl":terms_url,"purpose":purpose,"free":free,"termsAllowed":terms_allowed})

            }

        },

        Command::Requests { command } => match command {

            RequestsCommand::List => json!({"action":"inbox.list"}),

            RequestsCommand::Decide { id, decision } => {

                json!({"action":"inbox.decide","id":id,"decision":decision})

            }

        },

        Command::Bridge { command } => return run_bridge(command, &directory, cli.json).await,

    };

    let result = ipc::call(&directory, request).await?;

    if let Some(path) = output_file {

        use std::io::Write;

        let mut f = std::fs::OpenOptions::new()

            .write(true)

            .create_new(true)

            .open(&path)

            .with_context(|| format!("Cannot create {}", path.display()))?;

        f.write_all(&serde_json::to_vec_pretty(&result)?)?;

        return emit(&json!({"written":path}), cli.json);

    }

    emit(&result, cli.json)

}



fn transport_direct(

    direct: bool,

    relay_only: bool,

    relay: Option<&str>,

    dev_public: bool,

) -> Result<bool> {

    if relay_only && relay.is_none() && !dev_public {

        bail!("relay_only_requires_explicit_dev_public_relay_or_user_free_relay")

    }

    Ok(direct || (relay.is_none() && !dev_public))

}

fn read_json(path: &std::path::Path) -> Result<Value> {

    let metadata =

        std::fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;

    if metadata.len() > 512 * 1024 {

        bail!("json_file_too_large")

    }

    serde_json::from_slice(&std::fs::read(path)?)

        .with_context(|| format!("Invalid JSON in {}", path.display()))

}

fn file_request(action: &str, path: &std::path::Path) -> Result<Value> {

    let mut value = read_json(path)?;

    let object = value

        .as_object_mut()

        .context("request_file_must_be_object")?;

    object.insert("action".into(), json!(action));

    Ok(value)

}

fn descriptor_request(

    action: &str,

    file: Option<PathBuf>,

    skill_id: Option<String>,

    input: Option<PathBuf>,

) -> Result<Value> {

    let mut r = json!({"action":action});

    if let Some(path) = file {

        let v = read_json(&path)?;

        r["descriptor"] = v.get("descriptor").unwrap_or(&v).clone();

    }

    if let Some(id) = skill_id {

        r["skillId"] = json!(id)

    }

    if let Some(path) = input {

        r["input"] = read_json(&path)?

    }

    Ok(r)

}

fn owner_request(command: OwnerCommand, output: &mut Option<PathBuf>) -> Result<Value> {

    Ok(match command {

        OwnerCommand::Init => json!({"action":"owner.init"}),

        OwnerCommand::Inspect => json!({"action":"owner.inspect"}),

        OwnerCommand::Rotate => json!({"action":"owner.rotate"}),

        OwnerCommand::Backup { output: path } => {

            *output = path;

            json!({"action":"owner.backup"})

        }

        OwnerCommand::Restore {

            file,

            expected_owner,

        } => {

            let v = read_json(&file)?;

            json!({"action":"owner.restore","backup":v.get("backup").unwrap_or(&v),"expectedOwner":expected_owner})

        }

        OwnerCommand::DomainChallenge {

            domain,

            output: path,

        } => {

            *output = path;

            json!({"action":"owner.domain-challenge","domain":domain})

        }

        OwnerCommand::DomainVerify {

            domain,

            expected_owner,

        } => json!({"action":"owner.domain-verify","domain":domain,"expectedOwner":expected_owner}),

        OwnerCommand::DomainInspect { domain } => {

            json!({"action":"owner.domain-inspect","domain":domain})

        }

        OwnerCommand::Grant(a) => {

            *output = a.output;

            json!({"action":"owner.grant","agentId":a.agent_id,"audience":a.audience,"scopes":a.scopes,"ttlSeconds":a.ttl,"maxCalls":a.max_calls})

        }

        OwnerCommand::PairRequest { output: path } => {

            *output = path;

            json!({"action":"owner.pair-request"})

        }

        OwnerCommand::Pair {

            file,

            audience,

            scopes,

            ttl,

            max_calls,

            output: path,

        } => {

            *output = path;

            let v = read_json(&file)?;

            json!({"action":"owner.pair","request":v.get("request").unwrap_or(&v),"audience":audience,"scopes":scopes,"ttlSeconds":ttl,"maxCalls":max_calls})

        }

        OwnerCommand::RevocationExport { output: path } => {

            *output = path;

            json!({"action":"owner.revocation-export"})

        }

        OwnerCommand::RotationImport { file } => {

            let v = read_json(&file)?;

            json!({"action":"owner.rotation-import","transition":v.get("transition").unwrap_or(&v)})

        }

        OwnerCommand::Import {

            file,

            expected_owner,

            trust_owner,

        } => {

            let v = read_json(&file)?;

            json!({"action":"owner.import","grant":v.get("grant").unwrap_or(&v),"expectedOwner":expected_owner,"trustOwner":trust_owner})

        }

        OwnerCommand::Verify {

            grant_id,

            audience,

            scope,

        } => json!({"action":"owner.verify","grantId":grant_id,"audience":audience,"scope":scope}),

        OwnerCommand::Revoke { grant_id } => json!({"action":"owner.revoke","grantId":grant_id}),

        OwnerCommand::Revocations { file } => {

            let v = read_json(&file)?;

            json!({"action":"owner.revocations","document":v.get("document").or_else(||v.get("revocation")).unwrap_or(&v)})

        }

    })

}

fn directory_request(command: DirectoryCommand) -> Result<Value> {

    Ok(match command {

        DirectoryCommand::Add {

            url,

            id,

            terms_url,

            free,

            terms_allowed,

        } => {

            json!({"action":"directory.add","url":url,"id":id,"termsUrl":terms_url,"free":free,"termsAllowed":terms_allowed})

        }

        DirectoryCommand::List => json!({"action":"directory.list"}),

        DirectoryCommand::Remove { id } => json!({"action":"directory.remove","id":id}),

        DirectoryCommand::Sync { id } => json!({"action":"directory.sync","id":id}),

        DirectoryCommand::Import {

            file,

            source_id,

            trusted,

        } => {

            json!({"action":"directory.import","sourceId":source_id,"directory":read_json(&file)?,"trusted":trusted})

        }

    })

}

fn discovery_request(command: DiscoveryCommand) -> Result<Value> {

    Ok(match command {

        DiscoveryCommand::Lan { mode, seconds } => {

            json!({"action":"discovery.lan","mode":mode,"seconds":seconds})

        }

        DiscoveryCommand::Relay { command } => match command {

            DiscoveryRelayCommand::Add {

                url,

                terms_url,

                free,

                terms_allowed,

            } => {

                json!({"action":"discovery.relay.add","url":url,"termsUrl":terms_url,"free":free,"termsAllowed":terms_allowed})

            }

            DiscoveryRelayCommand::List => json!({"action":"discovery.relay.list"}),

            DiscoveryRelayCommand::Remove { id } => {

                json!({"action":"discovery.relay.remove","id":id})

            }

            DiscoveryRelayCommand::Probe { id } => {

                json!({"action":"discovery.relay.probe","id":id})

            }

        },

        DiscoveryCommand::Nostr { command } => match command {

            NostrCommand::Export { confirm_public, .. } => {

                json!({"action":"discovery.nostr.export","confirmPublic":confirm_public})

            }

            NostrCommand::Import { file, source_id } => {

                let v = read_json(&file)?;

                json!({"action":"discovery.nostr.import","event":v.get("event").unwrap_or(&v),"sourceId":source_id})

            }

            NostrCommand::Publish { id, confirm_public } => {

                json!({"action":"discovery.nostr.publish","id":id,"confirmPublic":confirm_public})

            }

            NostrCommand::Read { id, limit } => {

                json!({"action":"discovery.nostr.read","id":id,"limit":limit})

            }

        },

    })

}

fn channel_request(command: ChannelCommand) -> Result<Value> {

    Ok(match command {

        ChannelCommand::Create {

            name,

            visibility,

            max_members,

        } => {

            json!({"action":"channels.create","name":name,"visibility":visibility,"maxMembers":max_members})

        }

        ChannelCommand::List => json!({"action":"channels.list"}),

        ChannelCommand::Show { channel } => json!({"action":"channels.show","channelId":channel}),

        ChannelCommand::Invite { channel, peer } => {

            json!({"action":"channels.invite","channelId":channel,"peer":peer})

        }

        ChannelCommand::Join { invite } => json!({"action":"channels.join","invite":invite}),

        ChannelCommand::Publisher {

            channel,

            peer,

            can_publish,

        } => {

            json!({"action":"channels.publisher","channelId":channel,"peer":peer,"canPublish":can_publish})

        }

        ChannelCommand::Publish { channel, text, id } => {

            json!({"action":"channels.publish","channelId":channel,"text":text,"id":id})

        }

        ChannelCommand::Read {

            channel,

            source,

            after,

        } => json!({"action":"channels.read","channelId":channel,"sourceId":source,"after":after}),

        ChannelCommand::Subscribe {

            url,

            channel,

            source_id,

            expected_publisher,

            mirrors,

            wake,

        } => {

            json!({"action":"channels.subscribe","url":url,"channelId":channel,"sourceId":source_id,"expectedPublisher":expected_publisher,"mirrors":mirrors,"wake":wake})

        }

        ChannelCommand::Wake { source, enabled } => {

            json!({"action":"channels.wake","sourceId":source,"wake":enabled})

        }

        ChannelCommand::Subscriptions => json!({"action":"channels.subscriptions"}),

        ChannelCommand::Sync { source } => json!({"action":"channels.sync","sourceId":source}),

        ChannelCommand::Import { source, file } => {

            let bytes = std::fs::read(&file)?;

            if bytes.len() > 512 * 1024 {

                bail!("feed_file_too_large")

            }

            json!({"action":"channels.import","sourceId":source,"body":String::from_utf8(bytes).context("feed_must_be_utf8")?})

        }

        ChannelCommand::Unsubscribe { source } => {

            json!({"action":"channels.unsubscribe","sourceId":source})

        }

        ChannelCommand::Mute { source, muted } => {

            json!({"action":"channels.mute","sourceId":source,"muted":muted})

        }

        ChannelCommand::Export { .. } => bail!("channel_export_requires_local_output"),

        ChannelCommand::Watch { .. } => bail!("channel_watch_requires_poll_loop"),

    })

}

fn service_request(command: ServiceCommand) -> Result<Value> {

    Ok(match command {

        ServiceCommand::Inspect {

            provider,

            capability,

        } => json!({"action":"service.inspect","provider":provider,"capability":capability}),

        ServiceCommand::Quote { file } => file_request("service.quote", &file)?,

        ServiceCommand::Invoke { file } => file_request("service.invoke", &file)?,

        ServiceCommand::Status { job } => json!({"action":"service.status","jobId":job}),

        ServiceCommand::Result { job } => json!({"action":"service.result","jobId":job}),

        ServiceCommand::Cancel { job } => json!({"action":"service.cancel","jobId":job}),

        ServiceCommand::Review {

            job, approve, text, ..

        } => json!({"action":"service.review","jobId":job,"approve":approve,"text":text}),

        ServiceCommand::List { offset } => json!({"action":"service.list","offset":offset}),

    })

}

async fn run_bridge(

    command: BridgeCommand,

    directory: &std::path::Path,

    compact: bool,

) -> Result<()> {

    let (adapter, command) = match command {

        BridgeCommand::A2a { command } => ("a2a", command),

        BridgeCommand::Mcp { command } => ("mcp", command),

    };

    let (script, start, once, operation, extra) = match command {

        BridgeAdapterCommand::Registry { url, script } => (

            script,

            true,

            false,

            "mcp-registry",

            vec!["--url".into(), url.into()],

        ),

        BridgeAdapterCommand::Start {

            script,

            once,

            config,

        } => (

            script,

            true,

            once,

            adapter,

            if let Some(path) = config {

                vec![

                    "--config".into(),

                    std::path::absolute(path)?.into_os_string(),

                ]

            } else {

                Vec::<std::ffi::OsString>::new()

            },

        ),

        BridgeAdapterCommand::Status { script } => (script, false, false, adapter, Vec::new()),

        BridgeAdapterCommand::Serve { config, script } => (

            script,

            true,

            false,

            "a2a-serve",

            vec![

                "--config".into(),

                std::path::absolute(config)?.into_os_string(),

            ],

        ),

        BridgeAdapterCommand::Inspect {

            url,

            script,

            config,

        } => (script, true, false, "a2a-inspect", {

            let mut args: Vec<std::ffi::OsString> = vec!["--url".into(), url.into()];

            if let Some(path) = config {

                args.extend([

                    "--config".into(),

                    std::path::absolute(path)?.into_os_string(),

                ]);

            }

            args

        }),

    };

    if adapter == "mcp" && !matches!(operation, "mcp" | "mcp-registry") {

        bail!("serve_and_inspect_are_only_supported_by_a2a")

    }

    if adapter == "a2a" && operation == "mcp-registry" {

        bail!("registry_is_only_supported_by_mcp")

    }

    let script = script.unwrap_or_else(|| {

        let installed = std::env::current_exe()

            .ok()

            .and_then(|p| p.parent().map(|p| p.join("adapters/interop/bridge.mjs")));

        if let Some(path) = installed.as_ref().filter(|p| p.is_file()) {

            return path.clone();

        }

        let checkout = std::env::current_dir()

            .unwrap_or_else(|_| PathBuf::from("."))

            .join("adapters/interop/bridge.mjs");

        if checkout.is_file() {

            checkout

        } else {

            installed.unwrap_or(checkout)

        }

    });

    let script = std::path::absolute(script)?;

    if !start {

        return emit(

            &json!({"adapter":adapter,"script":script,"installed":script.is_file(),"running":"not_checked","lifecycle":"foreground","paidInfrastructure":false}),

            compact,

        );

    }

    if !script.is_file() {

        bail!(

            "bridge_not_installed: supply --script PATH to an installed adapter; no automatic npm installation"

        )

    }

    if adapter == "mcp" && once {

        bail!("once_is_only_supported_by_a2a_worker")

    }

    let mut child = tokio::process::Command::new("node");

    child

        .arg(script)

        .arg(operation)

        .arg("--data")

        .arg(directory)

        .args(extra);

    if once {

        child.arg("--once");

    }

    child.kill_on_drop(true);

    let status = child

        .status()

        .await

        .context("cannot_start_bridge: install Node and the adapter dependencies explicitly")?;

    if !status.success() {

        bail!("bridge_exited: {status}")

    }

    Ok(())

}



#[cfg(test)]

mod tests {

    use super::*;

    #[test]

    fn ttl_is_bounded_and_checked() {

        assert_eq!(parse_ttl("30m").unwrap(), 1800);

        assert_eq!(parse_ttl("1h").unwrap(), 3600);

        assert!(parse_ttl("0").is_err());

        assert!(parse_ttl("25h").is_err());

        assert!(parse_ttl("18446744073709551615h").is_err());

    }

    #[test]

    fn global_options_work_after_subcommands() {

        let cli = Cli::try_parse_from([

            "aiconnect",

            "temp",

            "create",

            "--ttl",

            "15m",

            "--data",

            "test-profile",

            "--json",

        ])

        .unwrap();

        assert!(cli.json);

        assert_eq!(cli.data.unwrap(), PathBuf::from("test-profile"));

    }



    #[test]

    fn incompatible_network_modes_are_rejected() {

        assert!(

            Cli::try_parse_from([

                "aiconnect",

                "daemon",

                "start",

                "--direct-only",

                "--relay-only"

            ])

            .is_err()

        );

    }

    #[test]

    fn free_only_transport_requires_explicit_public_test_opt_in() {

        assert!(transport_direct(false, false, None, false).unwrap());

        assert!(!transport_direct(false, false, None, true).unwrap());

        assert!(transport_direct(false, true, None, false).is_err());

        assert!(!transport_direct(false, true, Some("https://user-relay.example"), false).unwrap());

        assert!(

            Cli::try_parse_from(["aiconnect", "setup", "--direct-only", "--dev-public-relay"])

                .is_err()

        );

    }

    #[test]

    fn network_cli_keeps_publication_private_and_requires_free_terms() {

        let cli =

            Cli::try_parse_from(["aiconnect", "channel", "create", "--name", "Updates"]).unwrap();

        let Command::Channel { command } = cli.command else {

            panic!("channel expected")

        };

        let request = channel_request(command).unwrap();

        assert_eq!(request["visibility"], "private");

        assert!(

            Cli::try_parse_from([

                "aiconnect",

                "directory",

                "add",

                "https://example.org/directory.json",

                "--terms-url",

                "https://example.org/terms"

            ])

            .is_err()

        );

        assert!(

            Cli::try_parse_from([

                "aiconnect",

                "provider",

                "add",

                "https://example.org",

                "--terms-url",

                "https://example.org/terms",

                "--purpose",

                "directory",

                "--free",

                "--terms-allowed"

            ])

            .is_ok()

        );

    }

    #[test]

    fn delegation_cli_requires_owner_pin_and_scopes() {

        assert!(

            Cli::try_parse_from([

                "aiconnect",

                "owner",

                "import",

                "grant.json",

                "--expected-owner",

                "key"

            ])

            .is_err()

        );

        assert!(

            Cli::try_parse_from([

                "aiconnect",

                "owner",

                "grant",

                "agent",

                "--audience",

                "provider"

            ])

            .is_err()

        );

        let cli = Cli::try_parse_from([

            "aiconnect",

            "owner",

            "grant",

            "agent",

            "--audience",

            "provider",

            "--scopes",

            "service:invoke,read",

            "--ttl",

            "15m",

        ])

        .unwrap();

        let Command::Owner { command } = cli.command else {

            panic!("owner expected")

        };

        let r = owner_request(command, &mut None).unwrap();

        assert_eq!(r["ttlSeconds"], 900);

        assert_eq!(r["scopes"], json!(["service:invoke", "read"]));

        assert_eq!(r["maxCalls"], 1);

    }

    #[test]

    fn service_review_requires_one_explicit_decision() {

        assert!(Cli::try_parse_from(["aiconnect", "service", "review", "job"]).is_err());

        assert!(

            Cli::try_parse_from([

                "aiconnect",

                "service",

                "review",

                "job",

                "--approve",

                "--reject"

            ])

            .is_err()

        );

        let cli = Cli::try_parse_from([

            "aiconnect",

            "service",

            "review",

            "job",

            "--reject",

            "--text",

            "Wrong artifact",

        ])

        .unwrap();

        let Command::Service { command } = cli.command else {

            panic!("service expected")

        };

        let r = service_request(command).unwrap();

        assert_eq!(r["approve"], false);

        assert_eq!(r["text"], "Wrong artifact");

    }

    #[test]

    fn doctor_probes_only_with_explicit_network_flag() {

        assert!(Cli::try_parse_from(["aiconnect", "doctor", "--peer", "peer"]).is_err());

        let cli = Cli::try_parse_from(["aiconnect", "doctor"]).unwrap();

        assert!(matches!(

            cli.command,

            Command::Doctor {

                network: false,

                peer: None

            }

        ));

        let cli =

            Cli::try_parse_from(["aiconnect", "doctor", "--network", "--peer", "peer"]).unwrap();

        assert!(matches!(

            cli.command,

            Command::Doctor {

                network: true,

                peer: Some(_)

            }

        ));

    }

    #[test]

    fn channel_wake_is_explicit_and_separate_from_subscription() {

        let cli = Cli::try_parse_from(["aiconnect", "channel", "subscribe", "--channel", "group"])

            .unwrap();

        let Command::Channel { command } = cli.command else {

            panic!("channel expected")

        };

        assert_eq!(channel_request(command).unwrap()["wake"], false);

        let cli = Cli::try_parse_from([

            "aiconnect",

            "channel",

            "wake",

            "source",

            "--enabled",

            "false",

        ])

        .unwrap();

        let Command::Channel { command } = cli.command else {

            panic!("channel expected")

        };

        let r = channel_request(command).unwrap();

        assert_eq!(r["action"], "channels.wake");

        assert_eq!(r["wake"], false);

    }

}

