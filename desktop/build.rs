// Embedded locales, themes and artwork are read by macros at compile time.
fn main() {
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-changed=themes");
    println!("cargo:rerun-if-changed=assets");
}
