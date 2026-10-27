use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use clap::Args;
use clap::CommandFactory;
use clap::Parser;
use clap::Subcommand;
use clap_complete::Shell;
use frork_lib::assertions::Assertion;
use frork_lib::assertions::Status;
use frork_lib::compile;
use frork_lib::error::FrorkError;
use frork_lib::error::SharedError;
use frork_lib::report;
use frork_lib::report::Action;
use frork_lib::runtime::Declaration;
use frork_lib::runtime::StatusHandler;
use frork_lib::runtime::run_code;
use frork_lib::runtime::run_lua;
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
    /// Create, update, or remove whatever doesn't match its assertion.
    Apply(Source),
    /// Compile a script into a standalone executable.
    Compile {
        script: String,
        #[arg(short, long)]
        output: String,
        /// The Rust target triple to build for.
        #[arg(long, default_value = frork_lib::TARGET)]
        target: String,
        /// A frork binary for the target to build on, instead of this one
        /// (for the host) or the one published with this release.
        #[arg(long)]
        runner: Option<PathBuf>,
    },
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
            (Some(script), _) => Ok(run_script(script, handle_status)?),
            (None, Some(code)) => Ok(run_code(code, handle_status)?),
            (None, None) => unreachable!("clap requires a script or -e"),
        }
    }
}

/// The arguments of a compiled script, which runs in place of the CLI.
#[derive(Parser)]
#[command(name = "frork", version = frork_lib::VERSION)]
#[command(about = "Reports the assertions built into this script")]
struct Standalone {
    /// Create, update, or remove whatever doesn't match its assertion,
    /// instead of only reporting.
    #[arg(long)]
    apply: bool,

    /// Passed to the script in Lua's `arg` table, after `--`.
    #[arg(last = true)]
    args: Vec<String>,
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

    // A compiled script runs its payload in place of the CLI.
    if let Some(code) = payload()? {
        let standalone = Standalone::parse();
        let program = std::env::args().next().unwrap_or_default();
        let args = [vec![program], standalone.args].concat();
        return Ok(if standalone.apply {
            run_lua(&code, &args, apply)
        } else {
            run_lua(&code, &args, check)
        }?);
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
        Commands::Check(source) => source.run(check),
        Commands::Apply(source) => source.run(apply),
        Commands::Compile {
            script,
            output,
            target,
            runner,
        } => {
            let runner = match runner {
                Some(runner) => runner.clone(),
                None if target == frork_lib::TARGET => std::env::current_exe().into_diagnostic()?,
                None => compile::cached_runner(
                    &runner_cache()?,
                    compile::RELEASES_URL,
                    frork_lib::VERSION,
                    target,
                )?,
            };
            let runner = fs_err::read(runner).into_diagnostic()?;
            let exe = compile::bundle(&runner, &compile::compile_lua(script)?);
            fs_err::write(output, exe).into_diagnostic()?;
            fs_err::set_permissions(output, std::fs::Permissions::from_mode(0o755))
                .into_diagnostic()
        }
    }
}

fn payload() -> Result<Option<Vec<u8>>> {
    let exe = fs_err::File::open(std::env::current_exe().into_diagnostic()?).into_diagnostic()?;
    compile::payload(exe).into_diagnostic()
}

fn runner_cache() -> Result<PathBuf> {
    xdg::BaseDirectories::with_prefix("frork")
        .get_cache_home()
        .map(|cache| cache.join("runners"))
        .ok_or_else(|| miette!("No cache directory: set XDG_CACHE_HOME or HOME"))
}

fn check(
    declaration: Declaration,
    status: &Status,
    assertion: &dyn Assertion,
) -> Result<(), FrorkError> {
    print_lines(&report::render(declaration, status, assertion));
    Ok(())
}

fn apply(
    declaration: Declaration,
    status: &Status,
    assertion: &dyn Assertion,
) -> Result<(), FrorkError> {
    print_lines(&report::render(declaration, status, assertion));
    match report::action(declaration, status) {
        Action::Nothing => {}
        Action::Create => {
            assertion.create()?;
            println!("{}", report::CREATED);
        }
        Action::PromptUpdate => {
            if report::wants_update(&prompt(report::UPDATE_PROMPT)?) {
                assertion.update()?;
                println!("{}", report::UPDATED);
            } else {
                println!("{}", report::KEPT);
            }
        }
        Action::Remove => {
            assertion.remove()?;
            println!("{}", report::REMOVED);
        }
    }
    Ok(())
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

fn prompt(question: &str) -> Result<String, FrorkError> {
    use std::io::Write as _;

    print!("{question}");
    std::io::stdout().flush().map_err(|e| FrorkError::Prompt {
        step: "flush stdout",
        source: SharedError::new(e),
    })?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| FrorkError::Prompt {
            step: "read input",
            source: SharedError::new(e),
        })?;
    Ok(input)
}
