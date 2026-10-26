use clap::CommandFactory;
use clap::Parser;
use clap::Subcommand;
use clap_complete::Shell;
use frork_lib::assertions::AssertionType;
use frork_lib::assertions::Status;
use frork_lib::compile;
use frork_lib::compile::Sdk;
use frork_lib::report;
use frork_lib::runtime::run_code;
use frork_lib::runtime::run_script;
use miette::IntoDiagnostic as _;
use miette::Result;
use miette::miette;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

/// CalVer, set by build.rs — the crate version is not what ships.
const VERSION: &str = env!("FRORK_VERSION");
const TARGET: &str = env!("FRORK_TARGET");

#[derive(Parser)]
#[command(name = "frork", version = VERSION)]
#[command(about = "A Fennel-based configuration management tool")]
struct Cli {
    /// Generate shell completions and exit.
    #[arg(long, value_enum)]
    completions: Option<Shell>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Check {
        code: String,
    },
    Do {
        code: String,
    },
    Status {
        script: String,
    },
    Satisfy {
        script: String,
    },
    /// Compile a script into a standalone executable.
    Compile {
        script: String,
        #[arg(short, long)]
        output: String,
        /// The Rust target triple to build for.
        #[arg(long, default_value = TARGET)]
        target: String,
        /// A directory holding the target's SDK, unpacked from
        /// `bin/build-sdk`.
        #[arg(long)]
        sdk: std::path::PathBuf,
    },
}

fn main() -> Result<()> {
    // Assertion types panic through todo!() for cases that are not implemented
    // yet; this renders those with the same formatting as recoverable errors.
    miette::set_panic_hook();
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    if let Some(shell) = cli.completions {
        clap_complete::generate(shell, &mut Cli::command(), "frork", &mut std::io::stdout());
        return Ok(());
    }

    let Some(command) = cli.command else {
        Cli::command().print_help().into_diagnostic()?;
        return Ok(());
    };

    run(&command)
}

fn run(command: &Commands) -> Result<()> {
    match command {
        Commands::Check { code } => run_code(code, status),
        Commands::Do { code } => run_code(code, satisfy),
        Commands::Status { script } => run_script(script, status),
        Commands::Satisfy { script } => run_script(script, satisfy),
        Commands::Compile {
            script,
            output,
            target,
            sdk,
        } => {
            let sdk = Sdk::new(sdk);
            let cc = match std::env::var("CC") {
                Ok(cc) => cc,
                Err(_) => compile::zig_cc(target)?,
            };
            compile::compile_binary(script, output, &sdk, &cc)
        }
    }
}

fn status(status: &Status, assertion: &dyn AssertionType) -> Result<()> {
    print_lines(&report::render(status, assertion));
    Ok(())
}

fn satisfy(status: &Status, assertion: &dyn AssertionType) -> Result<()> {
    print_lines(&report::render(status, assertion));

    match status {
        Status::Ok => {}
        Status::Missing => {
            assertion.install()?;
            println!("ok: {assertion}");
        }
        Status::ConflictUpgrade(_) => {
            if report::wants_upgrade(&prompt("Upgrade? [y/N]: ")?) {
                assertion.upgrade()?;
                println!("ok: {assertion}");
            } else {
                println!("skipped: {assertion}");
            }
        }
    }
    Ok(())
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

fn prompt(question: &str) -> Result<String> {
    use std::io::Write as _;

    print!("{question}");
    std::io::stdout()
        .flush()
        .map_err(|e| miette!("Failed to flush stdout: {e}"))?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| miette!("Failed to read input: {e}"))?;
    Ok(input)
}
