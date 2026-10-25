//! Compiles the UI into the binary.
//!
//! 1. `blueprint-compiler` turns `data/ui/stable/*.blp` into `.ui` files in
//!    `OUT_DIR/ui/`.
//! 2. `glib-compile-resources` packs them, with `data/style.css`, into
//!    `OUT_DIR/corewatch.gresource`, which `main.rs` embeds with
//!    `gio::resources_register_include!`.
//!
//! 3. The last commit (hash, subject, body) goes to `OUT_DIR/commit.txt` for
//!    the "What's New" section of the About dialog: from `git` in a checkout,
//!    otherwise from `data/commit.txt`, which `git archive` fills in.
//!
//! The binary therefore needs no data files at run time, and a plain
//! `cargo build` produces a complete program.
//!
//! Tier overrides (0.3): a file in `data/ui/edge/` with the same name as one
//! in `data/ui/stable/` will replace it when the edge tier is selected. The
//! `edge` folder does not exist yet; nothing here depends on it.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let data = manifest.join("../../data");
    let stable = data.join("ui/stable");
    let out = PathBuf::from(env("OUT_DIR"));

    println!("cargo:rerun-if-changed={}", stable.display());
    println!(
        "cargo:rerun-if-changed={}",
        data.join("resources.gresource.xml").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        data.join("style.css").display()
    );

    let blueprints = blueprint_files(&stable);
    for file in &blueprints {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    compile_blueprints(&out.join("ui"), &stable, &blueprints);

    write_commit(&manifest.join("../.."), &data, &out);

    glib_build_tools::compile_resources(
        &[out.as_path(), data.as_path()],
        data.join("resources.gresource.xml")
            .to_str()
            .expect("data path is UTF-8"),
        "corewatch.gresource",
    );
}

/// Write `hash\nsubject\n\nbody` of the last commit, or an empty file when
/// it is not known (a plain source copy without `.git`).
fn write_commit(repo: &Path, data: &Path, out: &Path) {
    // Rebuild after every commit; a missing path would rerun every build.
    for file in [".git/HEAD", ".git/logs/HEAD"] {
        let path = repo.join(file);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    println!(
        "cargo:rerun-if-changed={}",
        data.join("commit.txt").display()
    );

    // Only this repository's own .git: a source tarball unpacked inside some
    // other checkout must not report that checkout's commit.
    let from_git = repo.join(".git").exists().then(|| {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["log", "-1", "--format=%H%n%s%n%n%b"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
    });
    let text = from_git
        .flatten()
        .or_else(|| std::fs::read_to_string(data.join("commit.txt")).ok())
        // Not filled in: a copy that did not come from git archive.
        .filter(|t| !t.starts_with("$Format"))
        .unwrap_or_default();
    std::fs::write(out.join("commit.txt"), text.trim_end())
        .unwrap_or_else(|e| panic!("cannot write commit.txt: {e}"));
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set by Cargo"))
}

/// Every `.blp` file in `dir`, sorted so builds are reproducible.
fn blueprint_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "blp"))
        .collect();
    files.sort();
    files
}

fn compile_blueprints(out_dir: &Path, input_dir: &Path, files: &[PathBuf]) {
    std::fs::create_dir_all(out_dir)
        .unwrap_or_else(|e| panic!("cannot create {}: {e}", out_dir.display()));
    let status = Command::new("blueprint-compiler")
        .arg("batch-compile")
        .arg(out_dir)
        .arg(input_dir)
        .args(files)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "cannot run blueprint-compiler ({e}). Install it: \
                 `sudo apt install blueprint-compiler` (Debian, Ubuntu), \
                 `sudo pacman -S blueprint-compiler` (Arch) or \
                 `sudo dnf install blueprint-compiler` (Fedora)"
            )
        });
    assert!(status.success(), "blueprint-compiler failed: {status}");
}
