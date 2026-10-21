// `sqlx::migrate!` embeds `migrations/`; rebuild when it changes.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
