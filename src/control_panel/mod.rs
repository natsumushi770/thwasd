use slint::ComponentHandle;
use std::{error::Error, rc::Rc};
use touhou_wasd::remapping::RemappingService;
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

slint::include_modules!();

pub fn run() -> Result<(), Box<dyn Error>> {
    let window = ControlPanel::new()?;
    let remapping = Rc::new(RemappingService::start()?);
    window.on_enabled_changed({
        let remapping = Rc::clone(&remapping);
        move |enabled| remapping.set_enabled(enabled)
    });

    // ウィンドウのイベントループ終了後、サービスを破棄して押下中のキーを解放する。
    window.run()?;
    remapping.set_enabled(false);
    Ok(())
}

pub fn show_error(message: &str) {
    let message: Vec<u16> = format!("Touhou WASD を起動できませんでした。\n\n{message}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let title: Vec<u16> = "Touhou WASD".encode_utf16().chain(Some(0)).collect();
    // 安全性: 両方の文字列は NUL 終端されており、ダイアログ表示中も有効である。
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
