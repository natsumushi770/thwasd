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

#[derive(Default)]
struct Control {
    enabled: AtomicBool,
    stopping: AtomicBool,
}

/// キーボードフックを管理する。破棄時は生成したキー入力を解放し、処理スレッドの終了を待つ。
pub struct RemappingService {
    control: Arc<Control>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

impl RemappingService {
    /// リマップを OFF の状態で起動する。有効化は GUI から明示的に行う。
    pub fn start() -> io::Result<Self> {
        let control = Arc::new(Control::default());
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("keyboard-remapping".into())
            .spawn({
                let control = Arc::clone(&control);
                move || run_worker(control, ready_tx)
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                control,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(error) => {
                let _ = worker.join();
                Err(io::Error::other(error.to_string()))
            }
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.control.enabled.store(enabled, Ordering::Release);
    }
}

impl Drop for RemappingService {
    fn drop(&mut self) {
        self.control.enabled.store(false, Ordering::Release);
        self.control.stopping.store(true, Ordering::Release);
        // キー入力がなくても、スレッド内のタイマーがメッセージループを起こす。
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

thread_local! {
    static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

struct HookState {
    control: Arc<Control>,
    remapper: Remapper,
    foreground: Option<(usize, u32)>,
    target: bool,
}

impl HookState {
    fn new(control: Arc<Control>) -> Self {
        let mut remapper = Remapper::default();
        for mapping in MAPPINGS {
            // 安全性: 有効な仮想キーコードを、低レベルフックのコールバック外で照会する。
            if unsafe { GetAsyncKeyState(mapping.source as i32) } < 0 {
                remapper.mark_already_held(mapping.source);
            }
        }
        Self {
            control,
            remapper,
            foreground: None,
            target: false,
        }
    }

    fn refresh_context(&mut self) {
        // 安全性: PID の出力先ポインターは有効であり、ウィンドウが null の場合も以下で処理する。
        let (window, process) = unsafe {
            let window = GetForegroundWindow();
            let mut process = 0;
            if !window.is_null() {
                GetWindowThreadProcessId(window, &mut process);
            }
            (window as usize, process)
        };
        let identity = (window != 0 && process != 0).then_some((window, process));
        if identity != self.foreground {
            // Windows が別プロセスにウィンドウハンドルを再利用した場合も、以前の変換状態を解除する。
            self.remapper.set_context(false, None, &mut send_output);
            self.foreground = identity;
            self.target = identity.is_some_and(|(_, process)| process_is_target(process));
        }
        let enabled = self.control.enabled.load(Ordering::Acquire)
            && !self.control.stopping.load(Ordering::Acquire);
        self.remapper
            .set_context(enabled, self.target.then_some(window), &mut send_output);
    }

    fn stop(&mut self) {
        self.remapper.set_context(false, None, &mut send_output);
    }
}

fn process_is_target(process_id: u32) -> bool {
    // 安全性: ハンドルはこのスコープ内だけで使用し、照会後に必ず閉じる。
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if process.is_null() {
            return false;
        }
        let mut path = vec![0u16; 32_768];
        let mut length = path.len() as u32;
        let success =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut length);
        CloseHandle(process);
        if success == 0 {
            return false;
        }
        let path = String::from_utf16_lossy(&path[..length as usize]);
        path.rsplit(['\\', '/'])
            .next()
            .is_some_and(is_target_process)
    }
}

fn send_output(output: Output) -> bool {
    let mut flags = KEYEVENTF_SCANCODE;
    if output.mapping.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if output.state == KeyState::Up {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: output.mapping.scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INPUT_TAG,
            },
        },
    };
    // 安全性: 初期化済みの INPUT と、ABI に一致する構造体サイズを渡す。
    unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) == 1 }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // 安全性: フックコードが非負の場合、Windows は有効な KBDLLHOOKSTRUCT のポインターを渡す。
        let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let ours = event.flags & LLKHF_INJECTED != 0 && event.dwExtraInfo == INPUT_TAG;
        if !ours {
            let state = match wparam as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeyState::Down),
                WM_KEYUP | WM_SYSKEYUP => Some(KeyState::Up),
                _ => None,
            };
            if let Some(key_state) = state {
                let suppress = HOOK_STATE.with(|cell| {
                    // SendInput でフックが再入する可能性があるため、Win32 コールバック内でパニックを起こさない。
                    let Ok(mut state) = cell.try_borrow_mut() else {
                        return false;
                    };
                    let Some(state) = state.as_mut() else {
                        return false;
                    };
                    state.refresh_context();
                    state
                        .remapper
                        .handle(event.vkCode, key_state, &mut send_output)
                });
                if suppress {
                    return 1;
                }
            }
        }
    }
    // 安全性: 未処理のイベントはそのまま次のフックへ渡す。この API は hhk を使用しない。
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    HOOK_STATE.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else {
            return;
        };
        if let Some(state) = state.as_mut() {
            if state
                .foreground
                .is_some_and(|(previous, _)| previous != window as usize)
            {
                // イベントは非同期なので、コールバック到着前にフォーカスが戻っていても、
                // 一時的なフォーカス喪失を反映して変換状態を解除する。
                state.remapper.set_context(false, None, &mut send_output);
            }
            state.refresh_context();
        }
    });
}

#[derive(Default)]
struct HookResources {
    keyboard: HHOOK,
    foreground: HWINEVENTHOOK,
    timer: usize,
}

impl HookResources {
    fn install() -> io::Result<Self> {
        let mut resources = Self::default();
        // 安全性: 静的なコールバックは、メッセージループが動作している間、このスレッド上で実行される。
        unsafe {
            let module = GetModuleHandleW(null_mut());
            if module.is_null() {
                return Err(io::Error::last_os_error());
            }
            resources.keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
            if resources.keyboard.is_null() {
                return Err(io::Error::last_os_error());
            }
            resources.foreground = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(foreground_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if resources.foreground.is_null() {
                return Err(io::Error::last_os_error());
            }
            resources.timer = SetTimer(null_mut(), 0, 16, None);
            if resources.timer == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(resources)
    }
}

impl Drop for HookResources {
    fn drop(&mut self) {
        HOOK_STATE.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                state.stop();
            }
        });
        // 安全性: null でない各リソースはこのスレッド上で作成され、それぞれ一度だけ解放する。
        unsafe {
            if self.timer != 0 {
                KillTimer(null_mut(), self.timer);
            }
            if !self.foreground.is_null() {
                UnhookWinEvent(self.foreground);
            }
            if !self.keyboard.is_null() {
                UnhookWindowsHookEx(self.keyboard);
            }
        }
    }
}

fn run_worker(control: Arc<Control>, ready: mpsc::SyncSender<io::Result<()>>) -> io::Result<()> {
    HOOK_STATE.with(|state| state.replace(Some(HookState::new(Arc::clone(&control)))));
    let resources = match HookResources::install() {
        Ok(resources) => resources,
        Err(error) => {
            let _ = ready.send(Err(io::Error::new(error.kind(), error.to_string())));
            HOOK_STATE.with(|state| state.replace(None));
            return Err(error);
        }
    };
    let _ = ready.send(Ok(()));
    let mut message = MSG::default();
    let result = loop {
        if control.stopping.load(Ordering::Acquire) {
            break Ok(());
        }
        // 安全性: メッセージキューと書き込み可能な MSG は、このスレッドが所有している。
        let status = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if status == -1 {
            break Err(io::Error::last_os_error());
        }
        if status == 0 {
            break Ok(());
        }
        if message.message == WM_TIMER {
            HOOK_STATE.with(|state| {
                if let Some(state) = state.borrow_mut().as_mut() {
                    state.refresh_context();
                }
            });
        } else {
            // 安全性: GetMessage が返したメッセージを、取得したスレッド上で配送する。
            unsafe {
                DispatchMessageW(&message);
            }
        }
    };
    drop(resources);
    HOOK_STATE.with(|state| state.replace(None));
    result
}
