use std::env;

fn main() {
    let api_endpoint = env::var("CARRICK_API_ENDPOINT")
        .unwrap_or_else(|_| "https://api.carrick.tools".to_string());

    println!("cargo:rustc-env=CARRICK_API_ENDPOINT={}", api_endpoint);
    println!("cargo:rerun-if-env-changed=CARRICK_API_ENDPOINT");
}
