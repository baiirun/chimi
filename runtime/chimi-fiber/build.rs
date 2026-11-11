fn main() {
    println!("cargo:rerun-if-changed=tests/generated_fixture.c");
    println!("cargo:rerun-if-changed=include/chimi_fiber.h");
    cc::Build::new()
        .file("tests/generated_fixture.c")
        .include("include")
        .flag_if_supported("-fno-omit-frame-pointer")
        .flag_if_supported("-fasynchronous-unwind-tables")
        .compile("chimi_fiber_fixture");
}
