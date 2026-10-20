use clap::CommandFactory;
use clap::Parser;
use clap::Subcommand;
use clap_complete::Shell;
use frork_cli::assertions::AssertionType;
use frork_cli::assertions::Status;
use frork_cli::report;
use frork_cli::runtime::compile_script;
use frork_cli::runtime::run_code;
use frork_cli::runtime::run_compiled;
use frork_cli::runtime::run_script;
use frork_cli::standalone;
use miette::IntoDiagnostic as _;
use miette::Result;
use miette::miette;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

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
    Build {
        script: String,
        #[arg(short, long)]
        output: String,
        /// A frork executable for the target platform to embed the script
        /// in. Defaults to this one.
        #[arg(long)]
        runtime: Option<String>,
    },
}

/// What a standalone binary accepts in place of the usual CLI.
#[derive(Parser)]
#[command(version = VERSION)]
struct Standalone {
    /// Install missing assertions instead of only reporting them.
    #[arg(long)]
    satisfy: bool,
}

fn main() -> Result<()> {
    // Assertion types panic through todo!() for cases that are not implemented
    // yet; this renders those with the same formatting as recoverable errors.
    miette::set_panic_hook();
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    if let Some(code) = standalone::embedded_script()? {
        let args = Standalone::parse();
        return if args.satisfy {
            run_compiled(&code, satisfy)
        } else {
            run_compiled(&code, status)
        };
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
        Commands::Build {
            script,
            output,
            runtime,
        } => build(script, output, runtime.as_deref()),
    }
}

fn build(script: &str, output: &str, runtime: Option<&str>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let lua_code = compile_script(script)?;
    let runtime = match runtime {
        Some(path) => path.into(),
        None => std::env::current_exe().into_diagnostic()?,
    };
    let exe = standalone::embed(fs_err::read(runtime).into_diagnostic()?, &lua_code)?;
    fs_err::write(output, exe).into_diagnostic()?;
    fs_err::set_permissions(output, std::fs::Permissions::from_mode(0o755)).into_diagnostic()?;
    println!("built {output}");
    Ok(())
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
