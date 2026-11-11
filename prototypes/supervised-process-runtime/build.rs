fn main() {
    println!("cargo:rerun-if-changed=c/heap.c");
    println!("cargo:rerun-if-changed=c/heap.h");
    cc::Build::new()
        .file("c/heap.c")
        .include("c")
        .flag_if_supported("-std=c11")
        .flag_if_supported("-Wall")
        .flag_if_supported("-Wextra")
        .compile("chimi_process_heap_prototype");
}
