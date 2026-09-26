use std::{env, process::Command};

fn main() {
    println!("cargo::rerun-if-changed=styles/tailwind.css");
    println!("cargo::rerun-if-changed=templates");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");

    let status = Command::new("bunx")
        .current_dir(&manifest_dir)
        .args([
            "@tailwindcss/cli",
            "-i",
            "./styles/tailwind.css",
            "-o",
            "./assets/main.css",
            "--minify",
        ])
        .status()
        .expect("Failed to execute Tailwind");

    if !status.success() {
        panic!("Tailwind CSS build failed");
    }
}
