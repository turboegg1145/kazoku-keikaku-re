fn main() {
    // pl_mpeg：单文件 C 解码器（MIT）。iOS 交叉编译时 cc 会自动找 xcrun clang。
    cc::Build::new()
        .file("vendor/kv_movie.c")
        .include("vendor")
        .warnings(false)
        .compile("kv_movie");
    println!("cargo:rerun-if-changed=vendor/kv_movie.c");
    println!("cargo:rerun-if-changed=vendor/pl_mpeg.h");
}
