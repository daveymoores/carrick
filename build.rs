use std::env;

fn main() {
    // Read environment variables at build time and set them as cargo environment variables.
    // If CARRICK_API_ENDPOINT is not set (e.g., in a local build), provide a default.
    let api_endpoint = env::var("CARRICK_API_ENDPOINT")
        .unwrap_or_else(|_| "https://default.carrick.io/api".to_string());

    println!("cargo:rustc-env=CARRICK_API_ENDPOINT={}", api_endpoint);

    // Handle GEMINI_API_KEY - provide a default for local builds
    let gemini_api_key =
        env::var("GEMINI_API_KEY").unwrap_or_else(|_| "default-gemini-key".to_string());

    println!("cargo:rustc-env=GEMINI_API_KEY={}", gemini_api_key);

    // Tell cargo to rerun this build script if the environment variables change.
    println!("cargo:rerun-if-env-changed=CARRICK_API_ENDPOINT");
    println!("cargo:rerun-if-env-changed=GEMINI_API_KEY");
}
