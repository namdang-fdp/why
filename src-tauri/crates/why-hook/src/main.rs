//! Entry point. All logic is in `lib.rs`; this file only enforces the
//! "print nothing, exit 0, whatever happens" contract around it.

fn main() {
    // The default panic hook prints the message to stderr, which is exactly
    // what a hook must never do. Replace it before anything else can panic.
    std::panic::set_hook(Box::new(|_| {}));
    // A panic unwinds up to here instead of exiting with status 101. (This
    // relies on `panic = "unwind"`, see the release profile in Cargo.toml.)
    let _ = std::panic::catch_unwind(why_hook::run);
    // Exit 0 explicitly, whatever happened. Exit 2 would block the agent.
    std::process::exit(0);
}
