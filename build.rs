fn main() {
    println!("cargo:rerun-if-changed=src/control_panel/window.slint");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        slint_build::compile("src/control_panel/window.slint")
            .expect("failed to compile the control panel");
    }
}
