// We need this feature as it changes `dylib` linking behavior and allows us to link to
// `rustc_driver`.
#![feature(rustc_private)]
// warn on lints, that are included in `rust-lang/rust`s bootstrap
#![warn(rust_2018_idioms, unused_lifetimes)]

extern crate rustc_driver;

mod argfile;

use std::env;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{self, Command, exit};

fn show_help() {
    if writeln!(&mut anstream::stdout().lock(), "{}", help_message()).is_err() {
        exit(rustc_driver::EXIT_FAILURE);
    }
}

fn show_version() {
    let version_info = rustc_tools_util::get_version_info!();
    if writeln!(&mut anstream::stdout().lock(), "{version_info}").is_err() {
        exit(rustc_driver::EXIT_FAILURE);
    }
}

pub fn main() {
    let args: Vec<_> = env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string().unwrap_or_else(|arg| {
                eprintln!(
                    "error: invalid UTF-8 in command-line argument: {}",
                    arg.as_encoded_bytes().escape_ascii()
                );
                process::exit(1);
            })
        })
        .collect();

    // Everything after `--` is passed to the compiler, including option values.
    let options = args.split(|arg| arg == "--").next().unwrap_or_default();
    if options.iter().any(|a| a == "--help" || a == "-h") {
        show_help();
        return;
    }

    if options.iter().any(|a| a == "--version" || a == "-V") {
        show_version();
        return;
    }

    if let Some(pos) = options.iter().position(|a| a == "--explain") {
        if let Some(lint) = options.get(pos + 1) {
            let lint = lint.to_ascii_lowercase();
            process::exit(clippy_lints::explain(
                &lint.strip_prefix("clippy::").unwrap_or(&lint).replace('-', "_"),
            ));
        }
        show_help();
        return;
    }

    if let Err(code) = process(args.into_iter().skip(1)) {
        process::exit(code);
    }
}

struct ClippyCmd {
    cargo_subcommand: &'static str,
    args: Vec<String>,
    clippy_args: Vec<String>,
}

impl ClippyCmd {
    fn new<I>(mut old_args: I) -> Self
    where
        I: Iterator<Item = String>,
    {
        let mut cargo_subcommand = "check";
        let mut args = vec![];
        let mut clippy_args: Vec<String> = vec![];

        for arg in old_args.by_ref() {
            match arg.as_str() {
                "--fix" => {
                    cargo_subcommand = "fix";
                    continue;
                },
                "--no-deps" => {
                    clippy_args.push("--no-deps".into());
                    continue;
                },
                "--" => break,
                _ => {},
            }

            args.push(arg);
        }

        clippy_args.append(&mut (old_args.collect()));
        if cargo_subcommand == "fix" && !clippy_args.iter().any(|arg| arg == "--no-deps") {
            clippy_args.push("--no-deps".into());
        }

        Self {
            cargo_subcommand,
            args,
            clippy_args,
        }
    }

    fn path() -> PathBuf {
        let mut path = env::current_exe()
            .expect("current executable path invalid")
            .with_file_name("clippy-driver");

        if cfg!(windows) {
            path.set_extension("exe");
        }

        path
    }

    fn into_std_cmd(self) -> Command {
        const SEPARATOR: &str = "__CLIPPY_HACKERY__";
        let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        // Keep the existing CLIPPY_ARGS representation when it can encode every argument.
        let clippy_args = if self
            .clippy_args
            .iter()
            .any(|arg| arg.is_empty() || arg.contains(SEPARATOR))
        {
            serde_json::to_string(&self.clippy_args).expect("serializing string arguments cannot fail")
        } else {
            self.clippy_args
                .iter()
                .fold(String::new(), |s, arg| s + arg + SEPARATOR)
        };

        let argfiles: Vec<_> = self
            .clippy_args
            .iter()
            .filter_map(|arg| arg.strip_prefix('@'))
            .collect();
        if !argfiles.is_empty() {
            let absolute = argfiles.iter().all(|path| Path::new(path).is_absolute());
            let root = if absolute {
                Some(PathBuf::new())
            } else {
                argfile_root(&cmd, &self.args)
            };
            let fingerprint = root.as_deref().and_then(|root| argfile::fingerprint(root, &argfiles));
            cmd.env(argfile::ENV, fingerprint.unwrap_or_default());
        }

        cmd.env("RUSTC_WORKSPACE_WRAPPER", Self::path())
            .env("CLIPPY_ARGS", clippy_args)
            .arg(self.cargo_subcommand)
            .args(&self.args);

        cmd
    }
}

/// Cargo normally launches the compiler at the workspace root, even from a member's subdirectory.
fn argfile_root(cargo: &Command, args: &[String]) -> Option<PathBuf> {
    let mut locate = Command::new(cargo.get_program());
    locate.args(["locate-project", "--workspace", "--message-format", "plain"]);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--manifest-path" {
            locate.arg(arg).arg(args.next()?);
        } else if arg.starts_with("--manifest-path=") {
            locate.arg(arg);
        }
    }
    let output = locate.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let manifest = std::str::from_utf8(&output.stdout).ok()?.trim_end();
    Some(Path::new(manifest).parent()?.to_owned())
}

fn process<I>(old_args: I) -> Result<(), i32>
where
    I: Iterator<Item = String>,
{
    let cmd = ClippyCmd::new(old_args);

    // The first compiler runs after build scripts. Keep its streamed arguments
    // available until Cargo has verified the suggestions it applied.
    let replay = if cmd.cargo_subcommand == "fix" && cmd.clippy_args.iter().any(|arg| arg.starts_with('@')) {
        Some(tempfile::tempdir().map_err(|error| {
            eprintln!("error: could not create Clippy argument-file directory: {error}");
            1
        })?)
    } else {
        None
    };
    let mut cmd = cmd.into_std_cmd();
    if let Some(dir) = &replay {
        cmd.env(argfile::FIX_DIR, dir.path());
    } else {
        cmd.env_remove(argfile::FIX_DIR);
    }

    let exit_status = cmd
        .spawn()
        .expect("could not run cargo")
        .wait()
        .expect("failed to wait for cargo?");

    if exit_status.success() {
        Ok(())
    } else {
        Err(exit_status.code().unwrap_or(-1))
    }
}

#[must_use]
pub fn help_message() -> &'static str {
    color_print::cstr!(
"Checks a package to catch common mistakes and improve your Rust code.

<green,bold>Usage</>:
    <cyan,bold>cargo clippy</> <cyan>[OPTIONS] [--] [<<ARGS>>...]</>

<green,bold>Common options:</>
    <cyan,bold>--no-deps</>                Run Clippy only on the given crate, without linting the dependencies
    <cyan,bold>--fix</>                    Automatically apply lint suggestions. This flag implies <cyan>--no-deps</> and <cyan>--all-targets</>
    <cyan,bold>-h</>, <cyan,bold>--help</>               Print this message
    <cyan,bold>-V</>, <cyan,bold>--version</>            Print version info and exit
    <cyan,bold>--explain [LINT]</>         Print the documentation for a given lint

See all options with <cyan,bold>cargo check --help</>.

<green,bold>Allowing / Denying lints</>

To allow or deny a lint from the command line you can use <cyan,bold>cargo clippy --</> with:

    <cyan,bold>-W</> / <cyan,bold>--warn</> <cyan>[LINT]</>       Set lint warnings
    <cyan,bold>-A</> / <cyan,bold>--allow</> <cyan>[LINT]</>      Set lint allowed
    <cyan,bold>-D</> / <cyan,bold>--deny</> <cyan>[LINT]</>       Set lint denied
    <cyan,bold>-F</> / <cyan,bold>--forbid</> <cyan>[LINT]</>     Set lint forbidden

You can use tool lints to allow or deny lints from your code, e.g.:

    <yellow,bold>#[allow(clippy::needless_lifetimes)]</>

<green,bold>Manifest Options:</>
    <cyan,bold>--manifest-path</> <cyan><<PATH>></>  Path to Cargo.toml
    <cyan,bold>--frozen</>                Require Cargo.lock and cache are up to date
    <cyan,bold>--locked</>                Require Cargo.lock is up to date
    <cyan,bold>--offline</>               Run without accessing the network
")
}
#[cfg(test)]
mod tests {
    use super::ClippyCmd;

    #[test]
    fn fix() {
        let args = "cargo clippy --fix".split_whitespace().map(ToString::to_string);
        let cmd = ClippyCmd::new(args);
        assert_eq!("fix", cmd.cargo_subcommand);
        assert!(!cmd.args.iter().any(|arg| arg.ends_with("unstable-options")));
    }

    #[test]
    fn fix_implies_no_deps() {
        let args = "cargo clippy --fix".split_whitespace().map(ToString::to_string);
        let cmd = ClippyCmd::new(args);
        assert!(cmd.clippy_args.iter().any(|arg| arg == "--no-deps"));
    }

    #[test]
    fn no_deps_not_duplicated_with_fix() {
        let args = "cargo clippy --fix -- --no-deps"
            .split_whitespace()
            .map(ToString::to_string);
        let cmd = ClippyCmd::new(args);
        assert_eq!(cmd.clippy_args.iter().filter(|arg| *arg == "--no-deps").count(), 1);
    }

    #[test]
    fn check() {
        let args = "cargo clippy".split_whitespace().map(ToString::to_string);
        let cmd = ClippyCmd::new(args);
        assert_eq!("check", cmd.cargo_subcommand);
    }
}
