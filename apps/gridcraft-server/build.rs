// A newly added migration must rebuild sqlx::migrate!'s embedded directory too.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
