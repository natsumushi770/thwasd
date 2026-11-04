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

/// 非同期のウィンドウ操作や入力配送が完了するまで待つ。タイムアウトの位置は呼び出し元として報告する。
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
    let mut app_process = Command::new(env!("CARGO_BIN_EXE_touhou-wasd"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    // 検証中にパニックが起きても、子プロセスを終了させてからパニックを再開する。
    let test_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let title = wide("Touhou WASD");
        let mut window = null_mut();
        wait_until(|| {
            assert!(
                app_process.try_wait().unwrap().is_none(),
                "application exited before showing a window"
            );
            let mut previous_window = null_mut();
            loop {
                // 安全性: NUL 終端のタイトルと直前の列挙結果を渡す。返されたハンドルの所有権は取得しない。
                let candidate_window = unsafe {
                    FindWindowExW(null_mut(), previous_window, null_mut(), title.as_ptr())
                };
                if candidate_window.is_null() {
                    return false;
                }
                let mut process_id = 0;
                // 安全性: 列挙したウィンドウを指定し、書き込み先には有効なローカル変数を渡す。
                unsafe {
                    GetWindowThreadProcessId(candidate_window, &mut process_id);
                }
                if process_id == app_process.id() {
                    window = candidate_window;
                    return true;
                }
                previous_window = candidate_window;
            }
        });
        // 安全性: このテストが起動したアプリのウィンドウにだけ、ウィンドウ操作コマンドを送る。
        unsafe {
            PostMessageW(window, WM_SYSCOMMAND, SC_MINIMIZE as usize, 0);
        }
        // 安全性: 管理下の子プロセスのウィンドウ状態を照会する。ハンドルの所有権は取得しない。
        wait_until(|| unsafe { IsIconic(window) } != 0);
        assert!(
            app_process.try_wait().unwrap().is_none(),
            "minimizing must not exit the app"
        );
        // 安全性: 管理下の子プロセスのウィンドウへ、ポインターを含まない復元コマンドを送る。
        unsafe {
            PostMessageW(window, WM_SYSCOMMAND, SC_RESTORE as usize, 0);
        }
        // 安全性: 管理下の子プロセスのウィンドウ状態だけを照会する。
        wait_until(|| unsafe { IsIconic(window) } == 0);
        // 安全性: 管理下の子プロセスのウィンドウへ、ポインターを含まない終了通知を送る。
        unsafe {
            PostMessageW(window, WM_CLOSE, 0, 0);
        }
        let mut exit_status = None;
        wait_until(|| {
            exit_status = app_process.try_wait().unwrap();
            exit_status.is_some()
        });
        assert!(exit_status.unwrap().success());
    }));
    let _ = app_process.kill();
    let _ = app_process.wait();
    if let Err(error) = test_result {
        std::panic::resume_unwind(error);
    }
}

/// 指定したプロセス名で動く受信専用ウィンドウと、その受信ログを管理する。
struct InputWindowFixture {
    fixture_process: Child,
    window: HWND,
    executable_path: PathBuf,
    log_path: PathBuf,
}

impl InputWindowFixture {
    /// 同じテスト実行ファイルを指定名でコピーして起動し、プロセス名による対象判定を実際に通す。
    /// 子プロセスでは input_window_fixture だけを実行し、独立したウィンドウへ入力を送れるようにする。
    fn start(process_name: &str) -> Self {
        let fixture_directory = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("windows-input-{}", std::process::id(),));
        fs::create_dir_all(&fixture_directory).unwrap();
        let executable_path = fixture_directory.join(process_name);
        let log_path = fixture_directory.join(format!("{process_name}.log"));
        fs::copy(std::env::current_exe().unwrap(), &executable_path).unwrap();
        let title = wide(&format!(
            "Touhou WASD test {} {process_name}",
            std::process::id()
        ));
        let fixture_process = Command::new(&executable_path)
            .args(["--ignored", "--exact", "input_window_fixture"])
            .env(
                "TOUHOU_TEST_TITLE",
                String::from_utf16_lossy(&title[..title.len() - 1]),
            )
            .env("TOUHOU_TEST_LOG", &log_path)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut fixture = Self {
            fixture_process,
            window: null_mut(),
            executable_path,
            log_path,
        };
        wait_until(|| {
            // 安全性: 有効な NUL 終端のタイトルを渡す。返されたハンドルの所有権は取得しない。
            fixture.window = unsafe { FindWindowW(null_mut(), title.as_ptr()) };
            !fixture.window.is_null()
        });
        fixture.activate();
        fixture
    }

    /// 検証用ウィンドウを前面にし、フォーカス移動とその通知処理を待つ。
    fn activate(&self) {
        // 安全性: この検証用オブジェクトが、操作対象のウィンドウを作成したプロセスを管理している。
        unsafe {
            SetForegroundWindow(self.window);
        }
        // 安全性: 引数のない照会で取得したハンドルを比較するだけで、所有権は取得しない。
        wait_until(|| unsafe { GetForegroundWindow() } == self.window);
        // テスト用キー入力を送る前に、非同期のアクティブウィンドウ変更通知の処理を待つ。
        thread::sleep(Duration::from_millis(50));
    }

    /// 受信済みの (メッセージ種別, 仮想キーコード, スキャンコード, 拡張キーか) をログから読む。
    fn events(&self) -> Vec<(u32, u32, u32, bool)> {
        let log_contents = fs::read_to_string(&self.log_path).unwrap_or_default();
        let mut events = Vec::new();
        for log_line in log_contents.lines() {
            let event_fields: Vec<u32> = log_line
                .split_whitespace()
                .filter_map(|value| value.parse().ok())
                .collect();
            if let [message_type, virtual_key, scan_code, extended_flag] = event_fields.as_slice() {
                events.push((*message_type, *virtual_key, *scan_code, *extended_flag != 0));
            }
        }
        events
    }

    /// 期待数のイベントが届いてから少し待ち、余分なイベントも含めて受信列全体を比較する。
    fn expect_events(&self, expected_events: &[(u32, u32, u32, bool)]) {
        wait_until(|| self.events().len() >= expected_events.len());
        thread::sleep(Duration::from_millis(30));
        assert_eq!(self.events(), expected_events);
    }
}

impl Drop for InputWindowFixture {
    fn drop(&mut self) {
        // 安全性: 管理下の検証用ウィンドウへ終了通知を送る。パニック時に備え、プロセスの強制終了も行う。
        unsafe {
            PostMessageW(self.window, WM_CLOSE, 0, 0);
        }
        let _ = self.fixture_process.kill();
        let _ = self.fixture_process.wait();
        let _ = fs::remove_file(&self.executable_path);
        let _ = fs::remove_file(&self.log_path);
        // このテストが作成したディレクトリだけを、再帰せずに削除する。
        let _ = fs::remove_dir(self.executable_path.parent().unwrap());
    }
}

/// 指定ウィンドウが前面であることを確認し、外部入力として元キーの押下または解放を送る。
fn input(recipient_window: HWND, virtual_key: u16, is_key_up: bool) {
    // ユーザーがフォーカスを切り替えても、無関係なアプリにテスト用キー入力を送らない。
    // 安全性: 引数のない照会で取得したハンドルを比較するだけで、所有権は取得しない。
    assert_eq!(unsafe { GetForegroundWindow() }, recipient_window);
    let synthetic_input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                // 安全性: テスト対象の有効な仮想キーコードを値で渡し、戻り値だけを使う。
                wScan: unsafe { MapVirtualKeyW(virtual_key as u32, MAPVK_VK_TO_VSC) } as u16,
                dwFlags: KEYEVENTF_SCANCODE | if is_key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // 安全性: 初期化済みの INPUT は有効である。識別値 0 で、アプリ外部からのキー入力を模擬する。
    assert_eq!(
        unsafe { SendInput(1, &synthetic_input, size_of::<INPUT>() as i32) },
        1
    );
}

/// パニック時もテスト用キーの解放と、テスト前のフォーカスの復元を試みる後片付け。
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
    // 安全性: 引数のない照会で取得したハンドルを復元用に記録するだけで、所有権は取得しない。
    let _desktop_restore = RestoreDesktop(unsafe { GetForegroundWindow() });
    let non_target_fixture = InputWindowFixture::start("not-a-target.exe");
    let target_fixture = InputWindowFixture::start("東方紅魔郷.exe");
    let service = RemappingService::start().unwrap();

    // OFF の間は、押下から解放まで元のキー入力をそのまま通す。
    input(target_fixture.window, b'W' as u16, false);
    input(target_fixture.window, b'W' as u16, true);
    let mut expected_events = vec![
        (WM_KEYDOWN, 0x57, 0x11, false),
        (WM_KEYUP, 0x57, 0x11, false),
    ];
    target_fixture.expect_events(&expected_events);
    service.set_enabled(true);

    // Win32 経由の各変換に、期待するスキャンコードと拡張キーフラグが設定されることを確認する。
    for (source_key, destination_virtual_key, destination_scan_code, is_extended) in [
        (b'W', 0x26, 0x48, true),
        (b'A', 0x25, 0x4b, true),
        (b'S', 0x28, 0x50, true),
        (b'D', 0x27, 0x4d, true),
        (b'J', 0x5a, 0x2c, false),
        (b'K', 0x58, 0x2d, false),
        (b'L', 0x10, 0x2a, false),
    ] {
        input(target_fixture.window, source_key as u16, false);
        input(target_fixture.window, source_key as u16, true);
        expected_events.push((
            WM_KEYDOWN,
            destination_virtual_key,
            destination_scan_code,
            is_extended,
        ));
        expected_events.push((
            WM_KEYUP,
            destination_virtual_key,
            destination_scan_code,
            is_extended,
        ));
        target_fixture.expect_events(&expected_events);
    }

    // 同時押しとキーリピートでは押下状態を維持し、OFF にすると両方のキーを解放する。
    input(target_fixture.window, b'W' as u16, false);
    input(target_fixture.window, b'J' as u16, false);
    input(target_fixture.window, b'W' as u16, false);
    expected_events.extend([
        (WM_KEYDOWN, 0x26, 0x48, true),
        (WM_KEYDOWN, 0x5a, 0x2c, false),
        (WM_KEYDOWN, 0x26, 0x48, true),
    ]);
    target_fixture.expect_events(&expected_events);
    // 安全性: 有効な仮想キーコードをコールバックの外で照会し、押下状態の最上位ビットだけを調べる。
    assert!(unsafe { GetAsyncKeyState(0x26) } < 0);
    assert!(unsafe { GetAsyncKeyState(0x5a) } < 0);
    service.set_enabled(false);
    expected_events.extend([(WM_KEYUP, 0x26, 0x48, true), (WM_KEYUP, 0x5a, 0x2c, false)]);
    target_fixture.expect_events(&expected_events);
    input(target_fixture.window, b'W' as u16, true);
    input(target_fixture.window, b'J' as u16, true);
    target_fixture.expect_events(&expected_events);

    // 対象ウィンドウの判定、フォーカス喪失時の解放、変換元の KeyUp の抑制を確認する。
    service.set_enabled(true);
    input(target_fixture.window, b'L' as u16, false);
    expected_events.push((WM_KEYDOWN, 0x10, 0x2a, false));
    target_fixture.expect_events(&expected_events);
    non_target_fixture.activate();
    // 安全性: 左 Shift の有効な仮想キーコードをコールバックの外で照会する。
    wait_until(|| unsafe { GetAsyncKeyState(0xa0) } >= 0);
    input(non_target_fixture.window, b'L' as u16, true);
    input(non_target_fixture.window, b'W' as u16, false);
    input(non_target_fixture.window, b'W' as u16, true);
    non_target_fixture.expect_events(&[
        (WM_KEYUP, 0x10, 0x2a, false),
        (WM_KEYDOWN, 0x57, 0x11, false),
        (WM_KEYUP, 0x57, 0x11, false),
    ]);

    // 変換先キーを押下中に終了すると、そのキーが解放され、通常の入力に戻る。
    target_fixture.activate();
    input(target_fixture.window, b'D' as u16, false);
    expected_events.push((WM_KEYDOWN, 0x27, 0x4d, true));
    target_fixture.expect_events(&expected_events);
    drop(service);
    expected_events.push((WM_KEYUP, 0x27, 0x4d, true));
    target_fixture.expect_events(&expected_events);
    // 安全性: 右矢印の有効な仮想キーコードをコールバックの外で照会する。
    assert!(unsafe { GetAsyncKeyState(0x27) } >= 0);
    input(target_fixture.window, b'D' as u16, true);
    expected_events.push((WM_KEYUP, 0x44, 0x20, false));
    target_fixture.expect_events(&expected_events);
}

/// InputWindowFixture が子プロセスで実行する受信側。環境変数で指定されたウィンドウを作り、キーイベントを記録する。
#[test]
#[ignore = "child process fixture for the explicit Windows input integration test"]
fn input_window_fixture() {
    let Ok(title) = std::env::var("TOUHOU_TEST_TITLE") else {
        return;
    };
    let mut event_log = fs::File::create(std::env::var_os("TOUHOU_TEST_LOG").unwrap()).unwrap();
    let title = wide(&title);
    let window_class_name = wide("TouhouWASDInputFixture");
    // 安全性: null は現在のプロセスのモジュールを指定する。返されたハンドルの所有権は取得しない。
    let module_handle = unsafe { GetModuleHandleW(null_mut()) };
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(DefWindowProcW),
        hInstance: module_handle,
        lpszClassName: window_class_name.as_ptr(),
        ..Default::default()
    };
    // 安全性: 構造体と NUL 終端のクラス名は有効で、ウィンドウプロシージャは静的な関数を指定する。
    assert_ne!(unsafe { RegisterClassW(&window_class) }, 0);
    // 安全性: 登録済みクラスと文字列は、ウィンドウ作成中も有効である。
    let window = unsafe {
        CreateWindowExW(
            0,
            window_class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            20,
            20,
            340,
            130,
            null_mut(),
            null_mut(),
            module_handle,
            null_mut(),
        )
    };
    assert!(!window.is_null());
    let mut message = MSG::default();
    // 安全性: このスレッドが検証用ウィンドウとメッセージキューを所有し、MSG の出力先も有効である。
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
                    event_log,
                    "{} {} {} {}",
                    message.message,
                    message.wParam,
                    (message.lParam >> 16) & 0xff,
                    (message.lParam >> 24) & 1
                )
                .unwrap();
                event_log.flush().unwrap();
            }
            DispatchMessageW(&message);
        }
        DestroyWindow(window);
    }
}
