fn main() {
    // Direct Cargo checks on a fresh checkout need no generated license texts.
    // Distribution packaging fills this directory before its release build.
    std::fs::create_dir_all("resources/licenses").expect("无法创建许可证资源目录");
    tauri_build::build();
}
