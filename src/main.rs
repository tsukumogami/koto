use clap::Parser;
use koto::cli::{report_failure, run, App};

fn main() {
    let app = App::parse();
    if let Err(e) = run(app) {
        std::process::exit(report_failure(&e));
    }
}
