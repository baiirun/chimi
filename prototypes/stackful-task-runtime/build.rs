fn main() {
    cc::Build::new()
        .file("c/generated_child.c")
        .flag_if_supported("-fno-omit-frame-pointer")
        .flag_if_supported("-funwind-tables")
        .compile("chimi_generated_child_prototype");
    println!("cargo:rerun-if-changed=c/generated_child.c");
}
