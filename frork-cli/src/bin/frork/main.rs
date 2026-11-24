use clap::Args;
use clap::CommandFactory;
use clap::Parser;
use clap::Subcommand;
use clap_complete::Shell;
use frork_lib::assertions::Provider;
use frork_lib::assertions::Status;
use frork_lib::report;
use frork_lib::runtime::StatusHandler;
use frork_lib::runtime::run_code;
use frork_lib::runtime::run_script;
use miette::IntoDiagnostic as _;
use miette::Result;
use miette::miette;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

#[derive(Parser)]
#[command(name = "frork", version = frork_lib::VERSION)]
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
    /// Report the status of each assertion.
    Check(Source),
    /// Create or update whatever doesn't match its assertion.
    Apply(Source),
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct Source {
    /// A Fennel script to evaluate.
    script: Option<String>,

    /// Fennel code to evaluate instead of a script.
    #[arg(short = 'e', long = "eval", value_name = "CODE")]
    code: Option<String>,
}

impl Source {
    fn run(&self, handle_status: impl StatusHandler) -> Result<()> {
        match (&self.script, &self.code) {
            (Some(script), _) => run_script(script, handle_status),
            (None, Some(code)) => run_code(code, handle_status),
            (None, None) => unreachable!("clap requires a script or -e"),
        }
    }
}

fn main() -> Result<()> {
    // Providers panic through todo!() for cases that are not implemented
    // yet; this renders those with the same formatting as recoverable errors.
    miette::set_panic_hook();
    // Warnings show without RUST_LOG, so they go to stderr to stay out of
    // the status lines on stdout.
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env_lossy();
    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(filter)
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
        Commands::Check(source) => source.run(check),
        Commands::Apply(source) => source.run(apply),
    }
}

fn check(status: &Status, assertion: &dyn Provider) -> Result<()> {
    print_lines(&report::render(status, assertion));
    Ok(())
}

fn apply(status: &Status, assertion: &dyn Provider) -> Result<()> {
    print_lines(&report::render(status, assertion));

    match status {
        Status::Present => {}
        Status::Absent => {
            assertion.create()?;
            print_lines(&report::render(&Status::Present, assertion));
        }
        Status::Drifted(_) => {
            if report::wants_update(&prompt(&report::update_prompt(assertion))?) {
                assertion.update()?;
                print_lines(&report::render(&Status::Present, assertion));
            } else {
                println!("{}", report::kept(assertion));
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
