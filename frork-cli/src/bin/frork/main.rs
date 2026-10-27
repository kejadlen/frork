use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use clap::CommandFactory;
use clap::Parser;
use clap::Subcommand;
use clap_complete::Shell;
use frork_lib::assertions::AssertionType;
use frork_lib::assertions::Status;
use frork_lib::compile;
use frork_lib::report;
use frork_lib::runtime::run_code;
use frork_lib::runtime::run_lua;
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
        /// A frork binary for the target to build on, instead of this one
        /// (for the host) or the one published with this release.
        #[arg(long)]
        runner: Option<PathBuf>,
    },
}

/// The arguments of a compiled script, which runs in place of the CLI.
#[derive(Parser)]
#[command(name = "frork", version = VERSION)]
#[command(about = "Reports the assertions built into this script")]
struct Standalone {
    /// Install missing assertions instead of only reporting them.
    #[arg(long)]
    satisfy: bool,

    /// Passed to the script in Lua's `arg` table, after `--`.
    #[arg(last = true)]
    args: Vec<String>,
}

fn main() -> Result<()> {
    // Assertion types panic through todo!() for cases that are not implemented
    // yet; this renders those with the same formatting as recoverable errors.
    miette::set_panic_hook();
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    // A compiled script runs its payload in place of the CLI.
    if let Some(code) = payload()? {
        return run_standalone(&code);
    }

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
            runner,
        } => {
            let runner = match runner {
                Some(runner) => runner.clone(),
                None if target == TARGET => std::env::current_exe().into_diagnostic()?,
                None => compile::Runners::default().get(VERSION, target)?,
            };
            let runner = fs_err::read(runner).into_diagnostic()?;
            let exe = compile::bundle(&runner, &compile::compile_lua(script)?);
            fs_err::write(output, exe).into_diagnostic()?;
            fs_err::set_permissions(output, std::fs::Permissions::from_mode(0o755))
                .into_diagnostic()
        }
    }
}

fn run_standalone(code: &[u8]) -> Result<()> {
    let Standalone {
        satisfy: install,
        args,
    } = Standalone::parse();
    let program = std::env::args().next().unwrap_or_default();
    let handle_status = move |state: &Status, assertion: &dyn AssertionType| {
        if install {
            satisfy(state, assertion)
        } else {
            status(state, assertion)
        }
    };
    run_lua(code, &program, &args, handle_status)
}

fn payload() -> Result<Option<Vec<u8>>> {
    let exe = fs_err::File::open(std::env::current_exe().into_diagnostic()?).into_diagnostic()?;
    compile::payload(exe).into_diagnostic()
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
