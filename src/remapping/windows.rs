//! 専用スレッドでのフック処理、アクティブウィンドウの検出、スキャンコード送信を行う Win32 アダプター。

use super::domain::{KeyState, MAPPINGS, Output, Remapper, is_target_process};
use std::{
    cell::RefCell,
    io,
    mem::size_of,
    ptr::null_mut,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
    },
    UI::{
        Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
            KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, SendInput,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetForegroundWindow,
            GetMessageW, GetWindowThreadProcessId, HHOOK, KBDLLHOOKSTRUCT, KillTimer,
            LLKHF_INJECTED, MSG, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
            WINEVENT_OUTOFCONTEXT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
        },
    },
};

// 32 ビット・64 ビット両方の ULONG_PTR に収まる識別値。このアプリが生成した入力だけを除外する。
const INPUT_TAG: usize = 0x5448_5744;

/// GUI から専用スレッドへ伝える操作要求。押下状態と Win32 リソースは専用スレッドだけで扱う。
#[derive(Default)]
struct WorkerRequests {
    remapping_enabled: AtomicBool,
    stop_requested: AtomicBool,
}

/// キーボードフックを管理する。破棄時は生成したキーの解放を試み、処理スレッドの終了を待つ。
pub struct RemappingService {
    worker_requests: Arc<WorkerRequests>,
    worker_thread: Option<JoinHandle<io::Result<()>>>,
}

impl RemappingService {
    /// リマップを OFF の状態で起動する。有効化は GUI から明示的に行う。
    /// フック・フォーカス監視・タイマーの導入が完了するまで待ち、導入時のエラーを返す。
    pub fn start() -> io::Result<Self> {
        let worker_requests = Arc::new(WorkerRequests::default());
        let (startup_result_tx, startup_result_rx) = mpsc::sync_channel(1);
        let worker_thread = thread::Builder::new()
            .name("keyboard-remapping".into())
            .spawn({
                let worker_requests = Arc::clone(&worker_requests);
                move || run_worker(worker_requests, startup_result_tx)
            })?;
        // スレッドの生成成功とフックの導入成功は別なので、導入結果を受け取ってから戻る。
        match startup_result_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                worker_requests,
                worker_thread: Some(worker_thread),
            }),
            Ok(Err(error)) => {
                let _ = worker_thread.join();
                Err(error)
            }
            Err(error) => {
                let _ = worker_thread.join();
                Err(io::Error::other(error.to_string()))
            }
        }
    }

    /// リマップの ON / OFF を要求する。ここでは要求を記録するだけで、入力は送らない。
    /// 実際の反映とキー解放は、専用スレッドの次の入力・フォーカス通知・タイマー処理で行う。
    pub fn set_enabled(&self, remapping_enabled: bool) {
        self.worker_requests
            .remapping_enabled
            .store(remapping_enabled, Ordering::Release);
    }
}

impl Drop for RemappingService {
    fn drop(&mut self) {
        self.worker_requests
            .remapping_enabled
            .store(false, Ordering::Release);
        self.worker_requests
            .stop_requested
            .store(true, Ordering::Release);
        // キー入力がなくても、スレッド内のタイマーがメッセージループを起こす。
        if let Some(worker_thread) = self.worker_thread.take() {
            let _ = worker_thread.join();
        }
    }
}

// コールバックからも同じ押下状態を参照できるよう、フックを導入したスレッドのローカル領域に置く。
thread_local! {
    static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

struct HookState {
    worker_requests: Arc<WorkerRequests>,
    remapper: Remapper,
    // 対象プロセスかどうかを判定した時点の、ウィンドウ識別値と PID。
    foreground_identity: Option<(usize, u32)>,
    // foreground_identity のプロセスが対象一覧に含まれているかをキャッシュする。
    foreground_is_target: bool,
}

impl HookState {
    /// 現時点の押下状態を読み、起動前から押されていたキーを途中から変換しない初期状態を作る。
    fn new(worker_requests: Arc<WorkerRequests>) -> Self {
        let mut remapper = Remapper::default();
        for mapping in MAPPINGS {
            // 安全性: 有効な仮想キーコードを、低レベルフックのコールバック外で照会する。
            if unsafe { GetAsyncKeyState(mapping.source_key as i32) } < 0 {
                remapper.mark_already_held(mapping.source_key);
            }
        }
        Self {
            worker_requests,
            remapper,
            foreground_identity: None,
            foreground_is_target: false,
        }
    }

    /// 現在のフォーカスと ON / OFF の要求を読み直し、ドメインの変換可否を更新する。
    /// プロセス名の照会はウィンドウまたは PID が変わったときだけ行う。
    /// 切り替えに伴うキー解放と、失敗していた解放の再試行も set_context 経由で行う。
    fn refresh_context(&mut self) {
        // 安全性: PID の出力先ポインターは有効であり、ウィンドウが null の場合も以下で処理する。
        let (foreground_window, process_id) = unsafe {
            let foreground_window = GetForegroundWindow();
            let mut process_id = 0;
            if !foreground_window.is_null() {
                GetWindowThreadProcessId(foreground_window, &mut process_id);
            }
            (foreground_window as usize, process_id)
        };
        let foreground_identity = if foreground_window != 0 && process_id != 0 {
            Some((foreground_window, process_id))
        } else {
            None
        };
        if foreground_identity != self.foreground_identity {
            // Windows が別プロセスにウィンドウハンドルを再利用した場合も、以前の変換状態を解除する。
            self.remapper.set_context(false, None, &mut send_output);
            self.foreground_identity = foreground_identity;
            self.foreground_is_target = match foreground_identity {
                Some((_, process_id)) => process_is_target(process_id),
                None => false,
            };
        }
        let remapping_enabled = self
            .worker_requests
            .remapping_enabled
            .load(Ordering::Acquire)
            && !self.worker_requests.stop_requested.load(Ordering::Acquire);
        let target_window = if self.foreground_is_target {
            Some(foreground_window)
        } else {
            None
        };
        self.remapper
            .set_context(remapping_enabled, target_window, &mut send_output);
    }

    /// 終了時に変換を中止し、押下中の変換先キーの解放を一度試みる。
    fn stop(&mut self) {
        self.remapper.set_context(false, None, &mut send_output);
    }
}

/// PID から実行ファイル名を取得して対象一覧と照合する。権限不足や終了済みで照会できない場合は対象外とする。
fn process_is_target(process_id: u32) -> bool {
    // 安全性: ハンドルはこのスコープ内だけで使用し、照会後に必ず閉じる。
    unsafe {
        let process_handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if process_handle.is_null() {
            return false;
        }
        let mut path_buffer = vec![0u16; 32_768];
        let mut path_length = path_buffer.len() as u32;
        let path_query_result = QueryFullProcessImageNameW(
            process_handle,
            PROCESS_NAME_WIN32,
            path_buffer.as_mut_ptr(),
            &mut path_length,
        );
        CloseHandle(process_handle);
        if path_query_result == 0 {
            return false;
        }
        let process_path = String::from_utf16_lossy(&path_buffer[..path_length as usize]);
        process_path
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(is_target_process)
    }
}

/// 変換出力をスキャンコードの合成入力として Windows の入力列へ挿入する。
/// 特定ウィンドウへの直接配送は行わず、Windows が 1 件の入力を受け付けた場合に true を返す。
fn send_output(output_event: Output) -> bool {
    let mut input_flags = KEYEVENTF_SCANCODE;
    if output_event.mapping.extended {
        input_flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if output_event.key_state == KeyState::Up {
        input_flags |= KEYEVENTF_KEYUP;
    }
    let synthetic_input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: output_event.mapping.scan_code,
                dwFlags: input_flags,
                time: 0,
                dwExtraInfo: INPUT_TAG,
            },
        },
    };
    // 安全性: 初期化済みの INPUT と、ABI に一致する構造体サイズを渡す。
    unsafe { SendInput(1, &synthetic_input, size_of::<INPUT>() as i32) == 1 }
}

/// 自アプリの合成入力を除外し、ドメインの判断に従って元イベントを抑制または通過させる。
/// フォーカス通知がまだ届いていない場合にも備え、元イベントの処理前に現在のフォーカスを確認する。
unsafe extern "system" fn keyboard_hook(hook_code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if hook_code >= 0 {
        // 安全性: フックコードが非負の場合、Windows は有効な KBDLLHOOKSTRUCT のポインターを渡す。
        let keyboard_event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let is_own_input =
            keyboard_event.flags & LLKHF_INJECTED != 0 && keyboard_event.dwExtraInfo == INPUT_TAG;
        if !is_own_input {
            let key_state = match wparam as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeyState::Down),
                WM_KEYUP | WM_SYSKEYUP => Some(KeyState::Up),
                _ => None,
            };
            if let Some(key_state) = key_state {
                let suppress_original_input = HOOK_STATE.with(|hook_state_cell| {
                    // SendInput でフックが再入する可能性がある。借用済みなら借用のパニックを避け、元イベントを通す。
                    let Ok(mut hook_state_guard) = hook_state_cell.try_borrow_mut() else {
                        return false;
                    };
                    let Some(hook_state) = hook_state_guard.as_mut() else {
                        return false;
                    };
                    hook_state.refresh_context();
                    hook_state
                        .remapper
                        .handle(keyboard_event.vkCode, key_state, &mut send_output)
                });
                if suppress_original_input {
                    return 1;
                }
            }
        }
    }
    // 安全性: 未処理のイベントはそのまま次のフックへ渡す。この API は hhk を使用しない。
    unsafe { CallNextHookEx(null_mut(), hook_code, wparam, lparam) }
}

/// キー入力がない間のフォーカス喪失も検知し、生成したキーの解放を試みる。
unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    notified_window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    HOOK_STATE.with(|hook_state_cell| {
        let Ok(mut hook_state_guard) = hook_state_cell.try_borrow_mut() else {
            return;
        };
        if let Some(hook_state) = hook_state_guard.as_mut() {
            if hook_state
                .foreground_identity
                .is_some_and(|(previous_window, _)| previous_window != notified_window as usize)
            {
                // イベントは非同期なので、コールバック到着前にフォーカスが戻っていても、
                // 一時的なフォーカス喪失を反映して変換状態を解除する。
                hook_state
                    .remapper
                    .set_context(false, None, &mut send_output);
            }
            hook_state.refresh_context();
        }
    });
}

/// 専用スレッド上で導入した Win32 リソースの所有者。
/// 導入の途中で失敗しても、取得済みのリソースは Drop で解放する。
#[derive(Default)]
struct HookResources {
    keyboard_hook: HHOOK,
    foreground_hook: HWINEVENTHOOK,
    timer_id: usize,
}

impl HookResources {
    /// キー入力・フォーカス変更のフックと、無入力時にも要求を処理するためのタイマーを導入する。
    fn install() -> io::Result<Self> {
        let mut hook_resources = Self::default();
        // 安全性: 静的なコールバックは、メッセージループが動作している間、このスレッド上で実行される。
        unsafe {
            let module_handle = GetModuleHandleW(null_mut());
            if module_handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            hook_resources.keyboard_hook =
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module_handle, 0);
            if hook_resources.keyboard_hook.is_null() {
                return Err(io::Error::last_os_error());
            }
            hook_resources.foreground_hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(foreground_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if hook_resources.foreground_hook.is_null() {
                return Err(io::Error::last_os_error());
            }
            // 入力がなくても OFF・終了要求や失敗したキー解放を処理できるよう、定期的にループを起こす。
            hook_resources.timer_id = SetTimer(null_mut(), 0, 16, None);
            if hook_resources.timer_id == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(hook_resources)
    }
}

impl Drop for HookResources {
    fn drop(&mut self) {
        // フックとタイマーを外す前に、生成したキーの解放を試みる。
        HOOK_STATE.with(|hook_state_cell| {
            if let Some(hook_state) = hook_state_cell.borrow_mut().as_mut() {
                hook_state.stop();
            }
        });
        // 安全性: null でない各リソースはこのスレッド上で作成され、それぞれ一度だけ解放する。
        unsafe {
            if self.timer_id != 0 {
                KillTimer(null_mut(), self.timer_id);
            }
            if !self.foreground_hook.is_null() {
                UnhookWinEvent(self.foreground_hook);
            }
            if !self.keyboard_hook.is_null() {
                UnhookWindowsHookEx(self.keyboard_hook);
            }
        }
    }
}

/// 初期化結果を呼び出し元へ返し、終了要求まで Win32 のメッセージループを動かす。
/// フックの導入・コールバックの処理・リソースの解放を、同じ専用スレッド上で行う。
fn run_worker(
    worker_requests: Arc<WorkerRequests>,
    startup_result_tx: mpsc::SyncSender<io::Result<()>>,
) -> io::Result<()> {
    HOOK_STATE.with(|hook_state_cell| {
        hook_state_cell.replace(Some(HookState::new(Arc::clone(&worker_requests))))
    });
    let hook_resources = match HookResources::install() {
        Ok(hook_resources) => hook_resources,
        Err(error) => {
            let _ = startup_result_tx.send(Err(io::Error::new(error.kind(), error.to_string())));
            HOOK_STATE.with(|hook_state_cell| hook_state_cell.replace(None));
            return Err(error);
        }
    };
    let _ = startup_result_tx.send(Ok(()));
    let mut message = MSG::default();
    let message_loop_result = loop {
        if worker_requests.stop_requested.load(Ordering::Acquire) {
            break Ok(());
        }
        // 安全性: メッセージキューと書き込み可能な MSG は、このスレッドが所有している。
        let message_status = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        // -1 は取得エラー、0 は WM_QUIT による通常終了を表す。
        if message_status == -1 {
            break Err(io::Error::last_os_error());
        }
        if message_status == 0 {
            break Ok(());
        }
        if message.message == WM_TIMER {
            HOOK_STATE.with(|hook_state_cell| {
                if let Some(hook_state) = hook_state_cell.borrow_mut().as_mut() {
                    hook_state.refresh_context();
                }
            });
        } else {
            // 安全性: GetMessage が返したメッセージを、取得したスレッド上で配送する。
            unsafe {
                DispatchMessageW(&message);
            }
        }
    };
    // リソースの Drop が押下状態を参照するため、HOOK_STATE を消す前に解放する。
    drop(hook_resources);
    HOOK_STATE.with(|hook_state_cell| hook_state_cell.replace(None));
    message_loop_result
}
