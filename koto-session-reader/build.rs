fn main() {
    // Spike trial: compile koto's shared source files in reader mode.
    println!("cargo:rustc-cfg=koto_reader");
    println!("cargo::rustc-check-cfg=cfg(koto_reader)");
}
