fn main() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let java_sdk_dir = manifest_dir.join("../../java-sdk");
    let jar_path = java_sdk_dir.join("build/conch-plugin-sdk.jar");

    // Tell cargo to re-run this build script when java-sdk sources change.
    if java_sdk_dir.join("src").exists() {
        println!("cargo:rerun-if-changed=../../java-sdk/src");
    }
    println!("cargo:rerun-if-changed=../../java-sdk/build/conch-plugin-sdk.jar");

    // Attempt to build the Java SDK JAR if make and a Makefile are available.
    if java_sdk_dir.join("Makefile").exists() {
        let status = std::process::Command::new("make")
            .arg("-C")
            .arg(&java_sdk_dir)
            .arg("build")
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                println!(
                    "cargo:warning=Java SDK build exited with {s} — Java plugins will be unavailable"
                );
            }
            Err(e) => {
                println!(
                    "cargo:warning=Could not run 'make' for Java SDK: {e} — Java plugins will be unavailable"
                );
            }
        }
    }

    // If the JAR exists, enable the real JVM module; otherwise the stub is used.
    if jar_path.exists() {
        println!("cargo:rustc-cfg=has_java_jar");
    } else {
        println!(
            "cargo:warning=Java SDK JAR not found at {} — building without JVM plugin support",
            jar_path.display()
        );
    }
}
