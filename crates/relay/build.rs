//! Rebuilds the crate when a migration changes: `sqlx::migrate!` embeds `migrations/` at compile
//! time, and a proc macro cannot tell Cargo to watch that directory itself.

// Cargo reads build-script directives from stdout: printing is the interface here, not logging.
#[allow(clippy::print_stdout)]
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
