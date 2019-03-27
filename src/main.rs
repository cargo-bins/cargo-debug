
use std::env;
use std::ffi::OsString;
use std::process::{Command, Stdio};

extern crate structopt;
use structopt::StructOpt;

#[macro_use]
extern crate log;

extern crate simplelog;
use simplelog::{TermLogger, LevelFilter};

use cargo_metadata::{Message, Artifact};

extern crate cargo_toml2;
use cargo_toml2::{CargoToml, from_path};


#[derive(StructOpt)]
#[structopt(name = "cargo-debug", about = "Cargo debug subcommand, wraps cargo invocations and launches a debugger")]
struct Options {

    #[structopt(default_value = "build")]
    /// Subcommand to invoke within cargo
    subcommand: String,

    #[structopt(long = "debugger", default_value = "gdb")]
    /// Debugger to launch as a subprocess
    debugger: String,

    #[structopt(long = "command-file")]
    /// Command file to be passed to debugger
    command_file: Option<String>,

    #[structopt(long = "log-level", default_value = "info")]
    /// Enable verbose logging
    level: LevelFilter,
}


fn main() {
    // Fetch args as an array for splitting
    let args: Vec<OsString> = env::args_os().map(|a| a ).collect();

    // Split options by "--" as debugger configuration or passthrough to cargo
    let mut s = args.splitn(3, |v| v == "--");
    let mut config_opts = match s.next() {
        Some(opts) => opts.iter().map(|a| a ).collect(),
        None => vec![],
    };

    // Filter out the first arg when run as a cargo subcommand
    if let Some(o) = config_opts.get(1).clone() {
        if o.to_str().unwrap() == "debug" {
            config_opts = config_opts.drain(2..).collect();
        }
    }

    let cargo_opts = s.next();
    let child_opts = s.next();

    // Load options
    let o = Options::from_iter(&config_opts);

    // Setup logging
    TermLogger::init(o.level, simplelog::Config::default()).unwrap();

    trace!("args: {:?}", args);
    trace!("cmd options: {:?}", config_opts);
    trace!("cargo options: {:?}", cargo_opts);
    trace!("child options: {:?}", child_opts);

    trace!("loading package file");

    let toml: CargoToml = from_path("Cargo.toml").expect("Failed to read Cargo.toml");

    let package = toml.package.name;

    trace!("found package: '{}'", package);

    trace!("building cargo command");

    // Build and execute cargo command
    let cargo_bin = env::var("CARGO").unwrap_or(String::from("cargo"));
    let mut cargo_cmd = Command::new(cargo_bin);
    cargo_cmd.arg(&o.subcommand);
    cargo_cmd.arg("--message-format=json");
    cargo_cmd.stdout(Stdio::piped());

    // Add no-run argument to test command
    if &o.subcommand == "test" {
        cargo_cmd.arg("--no-run");
    }

    // Attach additional arguments
    if let Some(opts) = cargo_opts {
        cargo_cmd.args(opts);
    }

    trace!("synthesized cargo command: {:?}", cargo_cmd);
    
    trace!("launching cargo command");
    let mut handle = cargo_cmd.spawn().expect("error starting cargo command");

    // Log all output artifacts
    let mut artifacts = vec![];
    for message in cargo_metadata::parse_messages(handle.stdout.take().unwrap()) {
        match message.expect("Invalid cargo JSON message") {
            Message::CompilerArtifact(artifact) => {
                artifacts.push(artifact);
            },
            _ => ()
        }
    }

    // Await command completion
    handle.wait().expect("cargo command failed, try running the command directly");
    trace!("command executed");

    // Find the output we care about
    let outputs: Vec<&Artifact> = artifacts.iter().filter(|a| a.target.name == package ).collect();
    trace!("found {} outputs: {:?}", outputs.len(), outputs);

    let bin = outputs.get(0).map(|o| o.executable.clone() ).expect("no output artifacts found").expect("output artifact does not contain binary file");

    info!("binary: {:?}", bin);

    let mut debug_cmd = Command::new(o.debugger);

    if let Some(_opts) = child_opts {
        // Forward child arguments if provided
        debug_cmd.arg("--args");
    }

    if let Some(command_file) = o.command_file {
        debug_cmd.arg("--command");
        debug_cmd.arg(command_file);
    }

    debug_cmd.arg(bin.into_os_string());

    if let Some(opts) = child_opts {
        debug_cmd.args(opts);
    }

    trace!("synthesized debug command: {:?}", debug_cmd);

    debug_cmd.status().expect("error running debug command");


}


#[cfg(test)]
mod test {
    #[test]
    fn fake_test() {
        assert!(true);
    }
}