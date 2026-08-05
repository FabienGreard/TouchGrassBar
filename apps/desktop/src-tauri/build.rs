fn main() {
    println!("cargo:rerun-if-env-changed=TOUCHGRASS_AUTH_SITE_URL");
    println!("cargo:rerun-if-env-changed=TOUCHGRASS_CONVEX_URL");
    tauri_build::build()
}
