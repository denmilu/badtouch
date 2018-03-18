#![warn(unused_extern_crates)]
extern crate hlua;
extern crate pbr;
extern crate threadpool;
extern crate colored;
extern crate time;
extern crate humantime;
extern crate atty;
extern crate rand;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate structopt;

extern crate reqwest;
extern crate mysql;
extern crate ldap3;

mod args;
mod ctx;
mod pb;
mod runtime;

use pb::ProgressBar;
use error_chain::ChainedError;
use threadpool::ThreadPool;
use colored::*;
use std::sync::mpsc;
use std::fs::{self, File};
use std::sync::Arc;
use std::time::Instant;
use std::io::{self, BufReader};
use std::io::prelude::*;

mod errors {
    use std;
    use hlua;

    error_chain! {
        foreign_links {
            Io(std::io::Error);
            Lua(hlua::LuaError);
        }
    }
}
use errors::{Result, ResultExt};

fn load_list(path: &str) -> Result<Vec<Arc<String>>> {
    let f = File::open(path)?;
    let file = BufReader::new(&f);
    let lines: io::Result<_> = file.lines()
            .map(|x| x.map(|x| Arc::new(x)))
            .collect();
    Ok(lines?)
}

fn load_scripts(paths: Vec<String>) -> Result<Vec<Arc<ctx::Script>>> {
    let mut scripts = Vec::new();

    for path in paths {
        let meta = fs::metadata(&path)?;

        if meta.is_dir() {
            for path in fs::read_dir(path)? {
                let path = path?.path();
                let path = path.to_str().unwrap();
                let script = Arc::new(ctx::Script::load(path)?);
                scripts.push(script);
            }
        } else {
            let script = Arc::new(ctx::Script::load(&path)?);
            scripts.push(script);
        }
    }

    Ok(scripts)
}

macro_rules! infof {
    ($arg1:tt, $fmt:expr, $($arg:tt)*) => (
        $arg1.bold().to_string() + " " + &(format!($fmt, $($arg)*).dimmed().to_string())
    );
}

macro_rules! info {
    ($arg1:tt, $fmt:expr, $($arg:tt)*) => (
        println!("{}", infof!($arg1, $fmt, $($arg)*));
    );
}

fn run() -> Result<()> {
    let args = args::parse();

    if atty::isnt(atty::Stream::Stdout) {
        colored::control::SHOULD_COLORIZE.set_override(false);
    }

    let users = load_list(&args.users).chain_err(|| "failed to load users")?;
    info!("[+]", "loaded {} users", users.len());
    let passwords = load_list(&args.passwords).chain_err(|| "failed to load passwords")?;
    info!("[+]", "loaded {} passwords", passwords.len());
    let scripts = load_scripts(args.scripts).chain_err(|| "failed to load scripts")?;
    info!("[+]", "loaded {} scripts", scripts.len());

    let attempts = users.len() * passwords.len() * scripts.len();

    let pool = ThreadPool::new(args.workers);
    let (tx, rx) = mpsc::channel();

    info!("[*]", "submitting {} jobs to threadpool with {} workers", attempts, args.workers);
    let start = Instant::now();
    for user in &users {
        for password in &passwords {
            for script in &scripts {
                let user = user.clone();
                let password = password.clone();
                let script = script.clone();
                let tx = tx.clone();
                pool.execute(move || {
                    let result = script.run_once(&user, &password);
                    tx.send((script, user, password, result)).expect("failed to send result");
                });
            }
        }
    }

    let mut pb = ProgressBar::new(attempts as u64);
    pb.tick();

    let mut valid = 0;
    for (script, user, password, result) in rx.iter().take(attempts) {
        match result {
            Ok(valid) if !valid => (),
            Ok(_) => {
                pb.writeln(format!("{} {}({}) => {:?}:{:?}", "[+]".bold(), "valid".green(), script.descr().yellow(), user, password));
                valid += 1;
            },
            Err(err) => {
                pb.writeln(format!("{} {}({}, {}): {:?}", "[!]".bold(), "error".red(), script.descr().yellow(), format!("{:?}:{:?}", user, password).dimmed(), err));
            }
        };
        pb.inc();
    }

    let elapsed = start.elapsed();
    let average = elapsed / attempts as u32;
    pb.finish_replace(infof!("[+]", "found {} valid credentials with {} attempts after {} and on average {} per attempt\n",
            valid, attempts,
            humantime::format_duration(elapsed),
            humantime::format_duration(average)));

    Ok(())
}

fn main() {
    if let Err(ref e) = run() {
        eprintln!("{}", e.display_chain());
        std::process::exit(1);
    }
}
