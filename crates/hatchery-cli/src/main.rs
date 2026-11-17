//! The `hatchery` command.

mod args;
mod inspect;
mod install;
mod pack;
mod publish;
mod registry;
mod verify;

use std::fmt;
use std::process::ExitCode;

use clap::Parser;

use crate::args::{Cli, Command};

/// An error to print as `error: …`.
#[derive(Debug)]
pub struct Fail(String);

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<hatchery::Error> for Fail {
    fn from(e: hatchery::Error) -> Fail {
        Fail(e.to_string())
    }
}

impl From<String> for Fail {
    fn from(s: String) -> Fail {
        Fail(s)
    }
}

impl From<&str> for Fail {
    fn from(s: &str) -> Fail {
        Fail(s.to_string())
    }
}

impl From<serde_json::Error> for Fail {
    fn from(e: serde_json::Error) -> Fail {
        Fail(e.to_string())
    }
}

/// The result of a command. The error is a message for the person running it, and `main` prints it after `error:`.
pub type Res<T = ()> = Result<T, Fail>;

/// Adds the path to an I/O error.
pub fn io<T>(r: std::io::Result<T>, path: &std::path::Path) -> Res<T> {
    r.map_err(|e| Fail(format!("{}: {e}", path.display())))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Res {
    let config = cli.config.as_deref();
    match cli.command {
        Command::Pack(a) => pack::run(a),
        Command::Inspect(a) => inspect::run(a),
        Command::Publish(a) => publish::run(a),
        Command::Withdraw(a) => publish::withdraw(a),
        Command::Verify(a) => verify::run(a),
        Command::Keygen => publish::keygen(),
        Command::Fetch(a) => install::fetch(config, a),
        Command::Add(a) => install::add(config, a),
        Command::Update(a) => install::update(config, a),
        Command::Install(a) => install::install(config, a),
        Command::Remove(a) => install::remove(config, a),
        Command::List(a) => install::list(config, a),
        Command::Registry(a) => registry::run(config, a),
    }
}

/// Lines of text in aligned columns.
pub fn table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for r in rows {
        let mut line = String::new();
        for (i, cell) in r.iter().enumerate() {
            line.push_str(cell);
            if i + 1 < r.len() {
                let pad = widths[i] - cell.chars().count() + 2;
                line.push_str(&" ".repeat(pad));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}
