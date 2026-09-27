use std::{env, process::Command};

fn main() {
    println!("cargo::rerun-if-changed=styles/tailwind.css");
    println!("cargo::rerun-if-changed=templates");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");

    let status = match Command::new("bunx")
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
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to execute Tailwind");
            eprintln!("{e:?}");
            panic!("Build process failed");
        }
    };

    if !status.success() {
        panic!("Tailwind CSS build failed");
    }
}
