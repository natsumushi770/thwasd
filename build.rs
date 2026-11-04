fn main() {
    println!("cargo:rerun-if-changed=src/control_panel/window.slint");
    println!("cargo:rerun-if-changed=assets/icon.png");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    // ビルドを行うホストではなく出力先の OS で判定し、Windows 向けの場合だけ GUI を生成する。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        slint_build::compile("src/control_panel/window.slint")
            .expect("failed to compile the control panel");
        // エクスプローラーやショートカットでも同じアイコンを使えるよう、実行ファイルに埋め込む。
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("failed to embed the application icon");
    }
}
