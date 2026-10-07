//! 明示的に実行するデスクトップ統合テスト。実ゲームではなく、独立した検証用ウィンドウを使う。
#![cfg(windows)]

use std::{
    fs,
    io::Write,
    mem::size_of,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    ptr::null_mut,
    thread,
    time::{Duration, Instant},
};
use touhou_wasd::remapping::RemappingService;
use windows_sys::Win32::{
    Foundation::HWND,
    System::{LibraryLoader::GetModuleHandleW, Threading::CREATE_NO_WINDOW},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
            KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW,
            GetForegroundWindow, GetMessageW, MSG, PostMessageW, RegisterClassW,
            SetForegroundWindow, WM_CLOSE, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        },
    },
};

use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowThreadProcessId, IsIconic, SC_MINIMIZE, SC_RESTORE, WM_SYSCOMMAND,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[track_caller]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "Windows input test timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "briefly opens the application window; run explicitly on a Windows desktop"]
fn control_panel_starts_minimizes_restores_and_exits() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_touhou-wasd"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let title = wide("Touhou WASD");
        let mut window = null_mut();
        wait_until(|| {
            assert!(
                child.try_wait().unwrap().is_none(),
                "application exited before showing a window"
            );
            let mut after = null_mut();
            loop {
                // 安全性: 指定タイトルに一致する最上位ウィンドウだけを調べる。
                let candidate =
                    unsafe { FindWindowExW(null_mut(), after, null_mut(), title.as_ptr()) };
                if candidate.is_null() {
                    return false;
                }
                let mut process_id = 0;
                unsafe {
                    GetWindowThreadProcessId(candidate, &mut process_id);
                }
                if process_id == child.id() {
                    window = candidate;
                    return true;
                }
                after = candidate;
            }
        });
        // 安全性: このテストが起動したアプリのウィンドウにだけ、ウィンドウ操作コマンドを送る。
        unsafe {
            PostMessageW(window, WM_SYSCOMMAND, SC_MINIMIZE as usize, 0);
        }
        wait_until(|| unsafe { IsIconic(window) } != 0);
        assert!(
            child.try_wait().unwrap().is_none(),
            "minimizing must not exit the app"
        );
        unsafe {
            PostMessageW(window, WM_SYSCOMMAND, SC_RESTORE as usize, 0);
        }
        wait_until(|| unsafe { IsIconic(window) } == 0);
        unsafe {
            PostMessageW(window, WM_CLOSE, 0, 0);
        }
        let mut exit_status = None;
        wait_until(|| {
            exit_status = child.try_wait().unwrap();
            exit_status.is_some()
        });
        assert!(exit_status.unwrap().success());
    }));
    let _ = child.kill();
    let _ = child.wait();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

struct Fixture {
    child: Child,
    window: HWND,
    executable: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn start(name: &str) -> Self {
        let directory = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("windows-input-{}", std::process::id(),));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join(name);
        let log = directory.join(format!("{name}.log"));
        fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        let title = wide(&format!("Touhou WASD test {} {name}", std::process::id()));
        let child = Command::new(&executable)
            .args(["--ignored", "--exact", "input_window_fixture"])
            .env(
                "TOUHOU_TEST_TITLE",
                String::from_utf16_lossy(&title[..title.len() - 1]),
            )
            .env("TOUHOU_TEST_LOG", &log)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut fixture = Self {
            child,
            window: null_mut(),
            executable,
            log,
        };
        wait_until(|| {
            // 安全性: 有効な NUL 終端のタイトルを渡す。返されたハンドルの所有権は取得しない。
            fixture.window = unsafe { FindWindowW(null_mut(), title.as_ptr()) };
            !fixture.window.is_null()
        });
        fixture.activate();
        fixture
    }

    fn activate(&self) {
        // 安全性: この検証用オブジェクトが、操作対象のウィンドウを作成したプロセスを管理している。
        unsafe {
            SetForegroundWindow(self.window);
        }
        wait_until(|| unsafe { GetForegroundWindow() } == self.window);
        // テスト用キー入力を送る前に、非同期のアクティブウィンドウ変更通知の処理を待つ。
        thread::sleep(Duration::from_millis(50));
    }

    fn events(&self) -> Vec<(u32, u32, u32, bool)> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let values: Vec<u32> = line
                    .split_whitespace()
                    .filter_map(|v| v.parse().ok())
                    .collect();
                (values.len() == 4).then(|| (values[0], values[1], values[2], values[3] != 0))
            })
            .collect()
    }

    fn expect_events(&self, expected: &[(u32, u32, u32, bool)]) {
        wait_until(|| self.events().len() >= expected.len());
        thread::sleep(Duration::from_millis(30));
        assert_eq!(self.events(), expected);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // 安全性: 管理下の検証用ウィンドウへ終了通知を送る。パニック時に備え、プロセスの強制終了も行う。
        unsafe {
            PostMessageW(self.window, WM_CLOSE, 0, 0);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.executable);
        let _ = fs::remove_file(&self.log);
        // このテストが作成したディレクトリだけを、再帰せずに削除する。
        let _ = fs::remove_dir(self.executable.parent().unwrap());
    }
}

fn input(window: HWND, key: u16, up: bool) {
    // ユーザーがフォーカスを切り替えても、無関係なアプリにテスト用キー入力を送らない。
    assert_eq!(unsafe { GetForegroundWindow() }, window);
    let event = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: unsafe { MapVirtualKeyW(key as u32, MAPVK_VK_TO_VSC) } as u16,
                dwFlags: KEYEVENTF_SCANCODE | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // 安全性: 初期化済みの INPUT は有効である。識別値 0 で、アプリ外部からのキー入力を模擬する。
    assert_eq!(
        unsafe { SendInput(1, &event, size_of::<INPUT>() as i32) },
        1
    );
}

struct RestoreDesktop(HWND);

impl Drop for RestoreDesktop {
    fn drop(&mut self) {
        // 安全性: テストで使う変換元キーだけを解放し、元のアクティブウィンドウに戻す。
        unsafe {
            for key in *b"WASDJKL" {
                let event = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: key as u16,
                            wScan: 0,
                            dwFlags: KEYEVENTF_KEYUP,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                SendInput(1, &event, size_of::<INPUT>() as i32);
            }
            if !self.0.is_null() {
                SetForegroundWindow(self.0);
            }
        }
    }
}

#[test]
#[ignore = "uses foreground desktop windows and synthesized keyboard input; run explicitly"]
fn remaps_only_foreground_targets_and_releases_held_keys() {
    let _restore = RestoreDesktop(unsafe { GetForegroundWindow() });
    let other = Fixture::start("not-a-target.exe");
    let target = Fixture::start("東方紅魔郷.exe");
    let service = RemappingService::start().unwrap();

    // OFF の間は、押下から解放まで元のキー入力をそのまま通す。
    input(target.window, b'W' as u16, false);
    input(target.window, b'W' as u16, true);
    let mut expected = vec![
        (WM_KEYDOWN, 0x57, 0x11, false),
        (WM_KEYUP, 0x57, 0x11, false),
    ];
    target.expect_events(&expected);
    service.set_enabled(true);

    // Win32 経由の各変換に、期待するスキャンコードと拡張キーフラグが設定されることを確認する。
    for (source, destination, scan, extended) in [
        (b'W', 0x26, 0x48, true),
        (b'A', 0x25, 0x4b, true),
        (b'S', 0x28, 0x50, true),
        (b'D', 0x27, 0x4d, true),
        (b'J', 0x5a, 0x2c, false),
        (b'K', 0x58, 0x2d, false),
        (b'L', 0x10, 0x2a, false),
    ] {
        input(target.window, source as u16, false);
        input(target.window, source as u16, true);
        expected.push((WM_KEYDOWN, destination, scan, extended));
        expected.push((WM_KEYUP, destination, scan, extended));
        target.expect_events(&expected);
    }

    // 同時押しとキーリピートでは押下状態を維持し、OFF にすると両方のキーを解放する。
    input(target.window, b'W' as u16, false);
    input(target.window, b'J' as u16, false);
    input(target.window, b'W' as u16, false);
    expected.extend([
        (WM_KEYDOWN, 0x26, 0x48, true),
        (WM_KEYDOWN, 0x5a, 0x2c, false),
        (WM_KEYDOWN, 0x26, 0x48, true),
    ]);
    target.expect_events(&expected);
    assert!(unsafe { GetAsyncKeyState(0x26) } < 0);
    assert!(unsafe { GetAsyncKeyState(0x5a) } < 0);
    service.set_enabled(false);
    expected.extend([(WM_KEYUP, 0x26, 0x48, true), (WM_KEYUP, 0x5a, 0x2c, false)]);
    target.expect_events(&expected);
    input(target.window, b'W' as u16, true);
    input(target.window, b'J' as u16, true);
    target.expect_events(&expected);

    // 対象ウィンドウの判定、フォーカス喪失時の解放、変換元の KeyUp の抑制を確認する。
    service.set_enabled(true);
    input(target.window, b'L' as u16, false);
    expected.push((WM_KEYDOWN, 0x10, 0x2a, false));
    target.expect_events(&expected);
    other.activate();
    wait_until(|| unsafe { GetAsyncKeyState(0xa0) } >= 0);
    input(other.window, b'L' as u16, true);
    input(other.window, b'W' as u16, false);
    input(other.window, b'W' as u16, true);
    other.expect_events(&[
        (WM_KEYUP, 0x10, 0x2a, false),
        (WM_KEYDOWN, 0x57, 0x11, false),
        (WM_KEYUP, 0x57, 0x11, false),
    ]);

    // 変換先キーを押下中に終了すると、そのキーが解放され、通常の入力に戻る。
    target.activate();
    input(target.window, b'D' as u16, false);
    expected.push((WM_KEYDOWN, 0x27, 0x4d, true));
    target.expect_events(&expected);
    drop(service);
    expected.push((WM_KEYUP, 0x27, 0x4d, true));
    target.expect_events(&expected);
    assert!(unsafe { GetAsyncKeyState(0x27) } >= 0);
    input(target.window, b'D' as u16, true);
    expected.push((WM_KEYUP, 0x44, 0x20, false));
    target.expect_events(&expected);
}

#[test]
#[ignore = "child process fixture for the explicit Windows input integration test"]
fn input_window_fixture() {
    let Ok(title) = std::env::var("TOUHOU_TEST_TITLE") else {
        return;
    };
    let mut log = fs::File::create(std::env::var_os("TOUHOU_TEST_LOG").unwrap()).unwrap();
    let title = wide(&title);
    let class = wide("TouhouWASDInputFixture");
    // 安全性: 静的なウィンドウプロシージャと、有効な NUL 終端のクラス名を渡す。
    let module = unsafe { GetModuleHandleW(null_mut()) };
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(DefWindowProcW),
        hInstance: module,
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    assert_ne!(unsafe { RegisterClassW(&window_class) }, 0);
    // 安全性: 登録済みクラスと文字列は、ウィンドウ作成中も有効である。
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            20,
            20,
            340,
            130,
            null_mut(),
            null_mut(),
            module,
            null_mut(),
        )
    };
    assert!(!window.is_null());
    let mut message = MSG::default();
    // 安全性: このスレッドが検証用ウィンドウとメッセージキューを作成し、所有している。
    unsafe {
        SetForegroundWindow(window);
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            if message.message == WM_CLOSE {
                break;
            }
            if matches!(
                message.message,
                WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP
            ) {
                writeln!(
                    log,
                    "{} {} {} {}",
                    message.message,
                    message.wParam,
                    (message.lParam >> 16) & 0xff,
                    (message.lParam >> 24) & 1
                )
                .unwrap();
                log.flush().unwrap();
            }
            DispatchMessageW(&message);
        }
        DestroyWindow(window);
    }
}
