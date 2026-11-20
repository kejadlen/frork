use std::process::Command;

fn main() {
    // Any rerun-if directive disables the rerun-on-package-change default, so
    // watch src to keep the dev fallback's date and change id fresh.
    println!("cargo:rerun-if-env-changed=FRORK_VERSION");
    println!("cargo:rerun-if-changed=src");
    let version = std::env::var("FRORK_VERSION").unwrap_or_else(|_| {
        let date = cmd("date", &["-u", "+%Y-%m-%d"]);
        let change = cmd(
            "jj",
            &["log", "-r", "@", "--no-graph", "-T", "change_id.short()"],
        );
        format!("{date}+{change}-dev")
    });
    println!("cargo:rustc-env=FRORK_VERSION={version}");
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
