use std::path::Path;
use std::process::Command;

fn npm() -> Command {
    if cfg!(target_os = "windows") {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "npm"]);
        cmd
    } else {
        Command::new("npm")
    }
}

fn run(mut cmd: Command, context: &str) {
    let status = cmd.status().unwrap_or_else(|e| panic!("{context}: {e}"));
    assert!(status.success(), "{context} failed with {status}");
}

fn main() {
    let frontend = Path::new("frontend");
    if !frontend.join("package.json").exists() {
        return;
    }

    let index_html = Path::new("src/ui/dist/index.html");

    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");
    println!("cargo:rerun-if-changed=frontend/tsconfig.json");
    println!("cargo:rerun-if-changed={}", index_html.display());

    if !frontend.join("node_modules").exists() {
        let mut cmd = npm();
        cmd.arg("install").current_dir(frontend);
        run(cmd, "npm install (web_ui frontend)");
    }

    if !index_html.exists() {
        let mut cmd = npm();
        cmd.args(["run", "build"]).current_dir(frontend);
        run(cmd, "npm run build (web_ui frontend)");
    }
}
