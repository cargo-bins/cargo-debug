
use std::env;
use std::time::{SystemTime, Duration};
use std::sync::{Arc, Mutex};
use std::ffi::OsString;
use std::process::{Command, Stdio};

extern crate structopt;
use structopt::StructOpt;

#[macro_use]
extern crate log;

extern crate simplelog;
use simplelog::{TermLogger, LevelFilter};

use cargo_metadata::{Message};

use cargo_manifest::{Manifest};


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

    #[structopt(long = "filter")]
    /// Filter to match against multiple output files
    filter: Option<String>,

    #[structopt(long = "no-run")]
    /// Print the debug command to the terminal and exit without running
    no_run: bool,

    #[structopt(long = "log-level", default_value = "info")]
    /// Enable verbose logging
    level: LevelFilter,
}


fn main() {
    // Fetch args as an array for splitting
    let args: Vec<OsString> = env::args_os().map(|a| a ).collect();

    println!("args: {:?}", args);

    // Split options by "--" as debugger configuration or passthrough to cargo
    let mut s = args.splitn(3, |v| v == "--");
    let mut config_opts = match s.next() {
        Some(opts) => opts.iter().map(|a| a ).collect(),
        None => vec![],
    };

    // Filter out the first arg when run as a cargo subcommand
    if let Some(o) = &config_opts.get(1).clone() {
        if o.to_str().unwrap() == "debug" {
            config_opts.remove(1);
        }
    }

    let cargo_opts: Option<Vec<_>> = match s.next() {
        Some(o) => Some(o.iter().map(|v| v.to_str().unwrap().to_string() ).collect()),
        None => None,
    };
    let child_opts: Option<Vec<_>> = match s.next() {
        Some(o) => Some(o.iter().map(|v| v.to_str().unwrap().to_string() ).collect()),
        None => None,
    };

    // Load options
    let o = Options::from_iter(&config_opts);

    // Setup logging
    TermLogger::init(o.level, simplelog::Config::default()).unwrap();

    trace!("args: {:?}", args);
    trace!("cmd options: {:?}", config_opts);
    trace!("cargo options: {:?}", cargo_opts);
    trace!("child options: {:?}", child_opts);

    trace!("loading package file");

    let toml: Manifest = Manifest::from_path("Cargo.toml").expect("Failed to read Cargo.toml");

    let package = toml.package.expect("No package available").name;

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

    // Find the output(s) we care about
    let outputs: Vec<_> = artifacts.iter().filter_map(|a| {      
        if let Some(x) = &a.executable {
            return Some(x.clone())
        }

        None
    } ).collect();
    trace!("found {} outputs: {:?}", outputs.len(), outputs);

    // Filter / select outputs
    let bin = match o.filter {
        Some(f) => {
            outputs.iter().find(|p| {
                let file_name = p.file_name().unwrap().to_str().unwrap();
                file_name.starts_with(&f)
            } ).expect("no fi")
        },
        None => {
            if outputs.len() > 1 {
                error!("found multiple output arguments, pass --filter=X argument to select a specific output");
                let names: Vec<_> = outputs.iter().filter_map(|o| o.file_name() ).collect();
                error!("{:#?}", names);
                return
            }

            outputs.get(0).expect("no viable output artifacts found")
        }
    };

    info!("selected binary: {:?}", bin);

    let debugger = o.debugger;

    let mut debug_args: Vec<String> = vec![];

    if debugger.ends_with("gdb") {
        // Prepare GDB to accept child options
        if let Some(_opts) = &child_opts {
            debug_args.push("--args".to_string());
        }

        // Append command file if provided
        if let Some(command_file) = o.command_file {
            debug_args.push("--command".to_string());
            debug_args.push(command_file);
        }

        // Specify file to be debugged
        debug_args.push(bin.clone().to_str().unwrap().to_string());

        // Append child options
        if let Some(opts) = &child_opts {
            debug_args.append(&mut opts.clone());
        }
    } else if debugger.ends_with("lldb") {
        // Specify file to be debugged
        debug_args.push("--file".to_string());
        debug_args.push(bin.clone().to_str().unwrap().to_string());

        // Append command file if provided
        if let Some(command_file) = o.command_file {
            debug_args.push("--source".to_string());
            debug_args.push(command_file);
        }

        // Append child options
        if let Some(opts) = child_opts {
            debug_args.push("--".to_string());
            debug_args.append(&mut opts.clone());
        }
    } else {
        error!("unsupported or unrecognised debugger {}", debugger);
        return;
    }

    trace!("synthesized debug arguments: {:?}", debug_args);

    if o.no_run {
        trace!("no-run selected, exiting");
        println!("Debug command: ");
        println!("{} {}", &debugger, debug_args.join(" "));
        std::process::exit(0);
    }

    let b = Arc::new(Mutex::new(SystemTime::now()));

    // Override ctrl+c handler to avoid premature exit
    // TODO: this... doesn't stop the rust process exiting..?
    ctrlc::set_handler(move || {
        warn!("CTRL+C");
        let mut then = b.lock().unwrap();
        let now = SystemTime::now();
        if now.duration_since(*then).unwrap() > Duration::from_secs(1) {
            std::process::exit(0);
        } else {
            *then = now;
        }
    }).expect("Error setting Ctrl-C handler");


    let mut debug_cmd = Command::new(&debugger);
    debug_cmd.args(debug_args);

    trace!("synthesized debug command: {:?}", debug_cmd);
    
    debug_cmd.status().expect("error running debug command");

    trace!("debug command done");
}


#[cfg(test)]
mod test {
    #[test]
    fn fake_test() {
        assert!(true);
    }
}
