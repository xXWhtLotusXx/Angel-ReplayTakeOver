fn main() {
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("exports.def");
    println!("cargo:rerun-if-changed={}", def.display());
    // Export the XInput 1.3 functions (including ordinal-only ones) under their real names.
    println!("cargo:rustc-cdylib-link-arg=/DEF:{}", def.display());
}
