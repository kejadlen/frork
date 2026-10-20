use clap::CommandFactory;
use clap::Parser;
use clap::Subcommand;
use clap_complete::Shell;
use frork_cli::assertions::AssertionType;
use frork_cli::assertions::Status;
use frork_cli::report;
use frork_cli::runtime::run_code;
use frork_cli::runtime::run_script;
use miette::IntoDiagnostic as _;
use miette::Result;
use miette::miette;

/// CalVer, set by build.rs — the crate version is not what ships.
const VERSION: &str = env!("FRORK_VERSION");

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
    Check { code: String },
    Do { code: String },
    Status { script: String },
    Satisfy { script: String },
}

fn main() -> Result<()> {
    // Assertion types panic through todo!() for cases that are not implemented
    // yet; this renders those with the same formatting as recoverable errors.
    miette::set_panic_hook();
    tracing_subscriber::fmt::init();

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
