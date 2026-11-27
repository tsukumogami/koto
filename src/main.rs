use clap::Parser;
use koto::cli::{report_failure, run, App};

fn main() {
    let app = App::parse();
    // Both arms end in the exit helper, so a command's exit hooks run on a
    // normal return as they do on an error.
    let code = match run(app) {
        Ok(()) => 0,
        Err(e) => report_failure(&e),
    };
    koto::exit::exit(code);
}
