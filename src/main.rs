use std::path::PathBuf;

use charles_local_mcp::{
    default_state_dir,
    mcp::McpServer,
    model::{DevicePlatform, Response, SetupPlanRequest},
    Service,
};
use clap::{Args, Parser, Subcommand};
use rmcp::{transport::stdio, ServiceExt};

#[derive(Debug, Parser)]
#[command(name = "charles-local-mcp", version, about)]
struct Cli {
    #[arg(long, global = true)]
    profiles_file: Option<PathBuf>,
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
    Doctor(JsonArgs),
    Profiles {
        #[command(subcommand)]
        command: ProfilesCommand,
    },
    Devices {
        #[command(subcommand)]
        command: DevicesCommand,
    },
    Setup {
        #[command(subcommand)]
        command: SetupCommand,
    },
    Status(JsonArgs),
    Cleanup {
        #[command(subcommand)]
        command: CleanupCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ProfilesCommand {
    List(JsonArgs),
    Validate(JsonArgs),
}

#[derive(Debug, Subcommand)]
enum DevicesCommand {
    List(JsonArgs),
}

#[derive(Debug, Subcommand)]
enum SetupCommand {
    Plan(SetupPlanArgs),
    Apply(TokenArgs),
    Resume(TokenArgs),
}

#[derive(Debug, Subcommand)]
enum CleanupCommand {
    Plan(JsonArgs),
    Apply(TokenArgs),
}

#[derive(Debug, Clone, Args)]
struct JsonArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
struct TokenArgs {
    #[arg(long)]
    token: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
struct SetupPlanArgs {
    #[arg(long)]
    profile: String,
    #[arg(long, value_enum, default_value_t = PlatformArg::Host)]
    platform: PlatformArg,
    #[arg(long)]
    device: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum PlatformArg {
    Host,
    Android,
    Ios,
}

impl From<PlatformArg> for DevicePlatform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Host => Self::Host,
            PlatformArg::Android => Self::Android,
            PlatformArg::Ios => Self::Ios,
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let state_dir = cli.state_dir.unwrap_or_else(default_state_dir);
    let service = match Service::new(state_dir, cli.profiles_file) {
        Ok(service) => service,
        Err(error) => {
            emit(&Response::error("startup", "state_error", error));
            std::process::exit(1);
        }
    };
    if matches!(cli.command, Command::Serve) {
        if let Err(error) = serve(service).await {
            eprintln!("charles-local-mcp server failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    let response = match cli.command {
        Command::Serve => unreachable!(),
        Command::Doctor(_) => service.doctor(),
        Command::Profiles { command } => match command {
            ProfilesCommand::List(_) => service.profiles_list(),
            ProfilesCommand::Validate(_) => service.profiles_validate(),
        },
        Command::Devices { command } => match command {
            DevicesCommand::List(_) => service.devices_list(),
        },
        Command::Setup { command } => match command {
            SetupCommand::Plan(arguments) => service.setup_plan(SetupPlanRequest {
                profile: arguments.profile,
                platform: arguments.platform.into(),
                device: arguments.device,
            }),
            SetupCommand::Apply(arguments) => service.setup_apply(&arguments.token),
            SetupCommand::Resume(arguments) => service.setup_resume(&arguments.token),
        },
        Command::Status(_) => service.status(),
        Command::Cleanup { command } => match command {
            CleanupCommand::Plan(_) => service.cleanup_plan(),
            CleanupCommand::Apply(arguments) => service.cleanup_apply(&arguments.token),
        },
    };
    emit(&response);
    if response.is_error() {
        std::process::exit(1);
    }
}

async fn serve(service: Service) -> Result<(), Box<dyn std::error::Error>> {
    let server = McpServer::new(service).serve(stdio()).await?;
    server.waiting().await?;
    Ok(())
}

fn emit(response: &Response) {
    println!(
        "{}",
        serde_json::to_string(response).expect("response serializes")
    );
}
