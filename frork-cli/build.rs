use std::process::Command;

fn main() {
    let version = std::env::var("FRORK_VERSION").unwrap_or_else(|_| {
        let date = cmd("date", &["-u", "+%Y-%m-%d"]);
        let change = cmd(
            "jj",
            &["log", "-r", "@", "--no-graph", "-T", "change_id.short()"],
        );
        format!("{date}+{change}-dev")
    });
    println!("cargo:rustc-env=FRORK_VERSION={version}");
    // The default target for `frork compile`: the platform frork runs on.
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=FRORK_TARGET={target}");
}

fn cmd(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}
