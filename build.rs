// Generates shell-completion scripts from the same clap command tree as the
// binary, so completions ship with the package and require no manual install
// step. The CLI definition lives in `src/cli.rs` and is shared with `main.rs`
// via `include!`; it is self-contained (no `use`/`mod`, no lib dependency) so
// it can be compiled here against the build-dependencies declared in
// Cargo.toml.
//
// Static completion (subcommands, flags, the memory-type enum, file paths via
// ValueHint) comes straight from clap_complete. Dynamic values (param/log
// variable names, flash targets) are added by appending the per-shell
// `completions/addendum.*` glue, which calls `cfcli __complete` at runtime.

use clap_complete::{generate_to, Shell};
use std::fs;
use std::path::{Path, PathBuf};

// The shared CLI definition. Fields are never read here (we only build the
// clap `Command`), so dead-code warnings are expected and suppressed.
#[allow(dead_code)]
mod cli {
    use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, ValueEnum, ValueHint};
    use clap_num::maybe_hex;
    use std::collections::HashMap;
    include!("src/cli.rs");

    pub fn command() -> clap::Command {
        CliArgs::command()
    }
}

fn append_file(target: &Path, addendum: &Path) {
    if let Ok(extra) = fs::read_to_string(addendum) {
        if let Ok(mut base) = fs::read_to_string(target) {
            base.push_str(&extra);
            let _ = fs::write(target, base);
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=src/cli.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=completions/addendum.bash");
    println!("cargo:rerun-if-changed=completions/addendum.zsh");
    println!("cargo:rerun-if-changed=completions/addendum.ps1");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = manifest.join("completions");
    fs::create_dir_all(&out_dir).expect("create completions dir");

    let mut cmd = cli::command();
    let bin = "cfcli";

    // Static scripts for every supported shell.
    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell] {
        generate_to(shell, &mut cmd, bin, &out_dir).expect("generate completion");
    }

    // --- bash: wrap the generated function to add dynamic candidates.
    append_file(&out_dir.join("cfcli.bash"), &out_dir.join("addendum.bash"));

    // --- zsh: point the param/log/flash-target argument actions at our
    // helper functions, then append the helper definitions. The matched
    // strings are the verbatim `Example:` lines / value names from the CLI
    // doc comments; if those change, completion silently falls back to the
    // default action (the build still succeeds).
    let zsh_path = out_dir.join("_cfcli");
    if let Ok(mut s) = fs::read_to_string(&zsh_path) {
        s = s.replace(
            "stabilizer.roll,stabilizer.pitch:_default",
            "stabilizer.roll,stabilizer.pitch:_cfcli_log_names",
        );
        s = s.replace(
            "usd.logging=1,loco.mode=2:_default",
            "usd.logging=1,loco.mode=2:_cfcli_param_set",
        );
        s = s.replace(
            "channel=10,address=E7E7E7E7E7,speed=2:_default",
            "channel=10,address=E7E7E7E7E7,speed=2:_cfcli_config_set",
        );
        s = s.replace(
            "loco.mode,kalman.initialX:_default",
            "loco.mode,kalman.initialX:_cfcli_param_names",
        );
        s = s.replace("::TARGETS:_default", "::TARGETS:_cfcli_flash_targets");
        s = s.replace(":BIN:_default", ":BIN:_cfcli_flash_bin");
        if let Ok(extra) = fs::read_to_string(out_dir.join("addendum.zsh")) {
            // The helper functions must be defined BEFORE clap's autoload
            // self-invocation (`_cfcli "$@"` near the end of the file), or
            // they are undefined on the first completion call. Insert them
            // just before the `_cfcli()` definition rather than appending.
            let anchor = "\n_cfcli() {";
            match s.find(anchor) {
                Some(pos) => s.insert_str(pos + 1, &format!("{}\n", extra.trim_end())),
                None => s.push_str(&extra),
            }
        }
        let _ = fs::write(&zsh_path, s);
    }

    // --- PowerShell: inject the dynamic block right before the final filter
    // that the generated completer applies to `$completions`.
    let ps_path = out_dir.join("_cfcli.ps1");
    if let (Ok(s), Ok(inject)) = (
        fs::read_to_string(&ps_path),
        fs::read_to_string(out_dir.join("addendum.ps1")),
    ) {
        let anchor = "    $completions.Where{";
        if let Some(pos) = s.find(anchor) {
            let mut out = String::with_capacity(s.len() + inject.len());
            out.push_str(&s[..pos]);
            out.push_str(&inject);
            out.push_str(&s[pos..]);
            let _ = fs::write(&ps_path, out);
        }
    }
}
