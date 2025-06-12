use std::env;

fn main() {
    // Read environment variables at build time and set them as cargo environment variables
    if let Ok(api_endpoint) = env::var("CARRICK_API_ENDPOINT") {
        println!("cargo:rustc-env=CARRICK_API_ENDPOINT={}", api_endpoint);
    } else {
        // Fallback to a default value or panic
        panic!("CARRICK_API_ENDPOINT environment variable must be set at build time");
    }

    if let Ok(lambda_url) = env::var("CARRICK_LAMBDA_URL") {
        println!("cargo:rustc-env=CARRICK_LAMBDA_URL={}", lambda_url);
    } else {
        // Fallback to a default value or panic
        panic!("CARRICK_LAMBDA_URL environment variable must be set at build time");
    }

    // Tell cargo to rerun this build script if these environment variables change
    println!("cargo:rerun-if-env-changed=CARRICK_API_ENDPOINT");
    println!("cargo:rerun-if-env-changed=CARRICK_LAMBDA_URL");
}