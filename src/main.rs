use clap::Parser;
use koto::cli::{run, App};

fn main() {
    let app = App::parse();
    if let Err(e) = run(app) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
