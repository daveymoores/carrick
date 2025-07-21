use std::env;

fn main() {
    // Read environment variables at build time and set them as cargo environment variables.
    // CARRICK_API_ENDPOINT is required at build time - no default provided.
    let api_endpoint = env::var("CARRICK_API_ENDPOINT")
        .expect("CARRICK_API_ENDPOINT environment variable must be set at build time");

    println!("cargo:rustc-env=CARRICK_API_ENDPOINT={}", api_endpoint);

    // Tell cargo to rerun this build script if the environment variables change.
    println!("cargo:rerun-if-env-changed=CARRICK_API_ENDPOINT");
}
