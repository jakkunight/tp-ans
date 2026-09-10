use std::{env, process::Command};

fn main() {
    println!("cargo::rerun-if-changed=styles/tailwind.css");
    println!("cargo::rerun-if-changed=templates");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");

    let tailwind = format!("tailwindcss");

    let status = Command::new(tailwind)
        .current_dir(&manifest_dir)
        .args([
            "-i",
            "./styles/tailwind.css",
            "-o",
            "./assets/main.css",
            "--minify",
        ])
        .status()
        .expect("failed to execute Tailwind");

    if !status.success() {
        panic!("Tailwind CSS build failed");
    }
}
