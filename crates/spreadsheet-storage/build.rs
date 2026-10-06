#![forbid(unsafe_code)]

fn main() {
    // SQLx embeds migrations at compile time. Track the directory so a newly
    // added migration also rebuilds the migrator on incremental local builds.
    println!("cargo:rerun-if-changed=../../migrations");
}
