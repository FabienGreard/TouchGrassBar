fn main() {
    println!("cargo:rerun-if-env-changed=CONVEX_SITE_URL");
    println!("cargo:rerun-if-env-changed=CONVEX_URL");
    println!("cargo:rerun-if-env-changed=TOUCHGRASS_DEV_KEYCHAIN_SERVICE");
    tauri_build::build()
}
