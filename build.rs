// Embed a manifest that asks Windows to run the exe elevated (works over admin windows).
fn main() {
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='requireAdministrator'");
}
