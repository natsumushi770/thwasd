//! Win32 や UI に依存しないリマップ規則とキー押下状態の管理。
//!
//! 元キーの押下ごとに入力を通すか変換するかを判断し、必要な出力を emit に渡す。
//! 入力の送信と対象ウィンドウの検出は、呼び出し側のアダプターが担当する。

/// 入力元の仮想キーコードと、変換先の物理キーを表すスキャンコードの組。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mapping {
    pub source: u32,
    pub scan_code: u16,
    /// 矢印キーを、同じスキャンコードを持つテンキーと区別するためのフラグ。
    pub extended: bool,
}

pub(super) const MAPPINGS: [Mapping; 7] = [
    Mapping {
        source: b'W' as u32,
        scan_code: 0x48,
        extended: true,
    },
    Mapping {
        source: b'A' as u32,
        scan_code: 0x4b,
        extended: true,
    },
    Mapping {
        source: b'S' as u32,
        scan_code: 0x50,
        extended: true,
    },
    Mapping {
        source: b'D' as u32,
        scan_code: 0x4d,
        extended: true,
    },
    Mapping {
        source: b'J' as u32,
        scan_code: 0x2c,
        extended: false,
    },
    Mapping {
        source: b'K' as u32,
        scan_code: 0x2d,
        extended: false,
    },
    Mapping {
        source: b'L' as u32,
        scan_code: 0x2a,
        extended: false,
    },
];

const TARGET_PROCESS_NAMES: [&str; 23] = [
    "東方紅魔郷.exe",
    "th07.exe",
    "th08.exe",
    "th09.exe",
    "th095.exe",
    "th10.exe",
    "th11.exe",
    "th12.exe",
    "th125.exe",
    "th128.exe",
    "th13.exe",
    "th14.exe",
    "th143.exe",
    "th15.exe",
    "th16.exe",
    "th165.exe",
    "th17.exe",
    "th18.exe",
    "th185.exe",
    "th19.exe",
    "th20.exe",
    "th06nc.exe",
    "th06c.exe",
];

/// パスではなくファイル名を受け取り、対象一覧と ASCII の大文字・小文字を区別せずに完全一致で照合する。
pub(super) fn is_target_process(name: &str) -> bool {
    TARGET_PROCESS_NAMES
        .iter()
        .any(|target| name.eq_ignore_ascii_case(target))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyState {
    Down,
    Up,
}

/// 呼び出し側へ送信を依頼する、変換先キーの押下または解放。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Output {
    pub mapping: Mapping,
    pub state: KeyState,
}

/// 元キーの KeyDown から KeyUp までの扱い。途中で対象が変わっても、元入力を通す方針へは切り替えない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SourceState {
    /// 押下を追跡していない。次の KeyDown で変換するかどうかを決める。
    #[default]
    Idle,
    /// 元入力として通した押下。変換が有効になっても KeyUp まではそのまま通す。
    Forwarded,
    /// 変換先の KeyDown を送信済み。元入力を抑制し、キーリピートも変換する。
    Remapped,
    /// フォーカス喪失などで変換を中止した押下。元キーの KeyUp までは元入力を抑制する。
    Canceled,
}

// 元キーが離されても、変換先の KeyUp 送信に失敗している場合がある。
// 元キーの押下の扱いと、変換先の解放待ちは独立して保持する。
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    source: SourceState,
    // KeyDown は送信できたが、対応する KeyUp の送信がまだ成功していない。
    output_down: bool,
}

/// 変換を許可するウィンドウと、各元キーの押下から解放までの状態を管理する。
/// 各メソッドの emit は、出力を送信できた場合に true を返す。
#[derive(Default)]
pub(super) struct Remapper {
    // None は OFF または対象外。ウィンドウの識別値は前回との比較だけに使う。
    active_window: Option<usize>,
    // MAPPINGS と同じ順番で、変換元キーごとに独立した状態を持つ。
    slots: [Slot; MAPPINGS.len()],
}

impl Remapper {
    /// リマップの有効状態と対象ウィンドウを更新する。target_window が None なら対象外とする。
    ///
    /// OFF・対象外への移動・対象ウィンドウの変更時には、生成したキーの解放を試みる。
    /// 変換を中止した元キーは、離されるまで元の入力を抑制する。
    /// 状態に変更がなくても、前回失敗したキー解放を再試行する。
    pub fn set_context(
        &mut self,
        enabled: bool,
        target_window: Option<usize>,
        emit: &mut impl FnMut(Output) -> bool,
    ) {
        let active_window = target_window.filter(|_| enabled);
        if active_window != self.active_window {
            self.active_window = active_window;
            for slot in &mut self.slots {
                if slot.source == SourceState::Remapped {
                    slot.source = SourceState::Canceled;
                }
            }
        }
        self.retry_releases(emit);
    }

    /// 起動時から押下中の元キーを、元入力として通過済みの押下として記録する。
    /// 通常の入力処理を始める前に呼び出し、最初の KeyUp までは途中から変換しない。
    pub fn mark_already_held(&mut self, source: u32) {
        if let Some(index) = MAPPINGS.iter().position(|mapping| mapping.source == source) {
            self.slots[index].source = SourceState::Forwarded;
        }
    }

    /// 元キーの KeyDown / KeyUp を処理し、必要な変換先イベントを emit に渡す。
    ///
    /// true を返した場合、呼び出し側は元イベントの配送を止める。
    /// 最初の KeyDown の送信に失敗した場合、その押下は KeyUp まで元入力として通す。
    pub fn handle(
        &mut self,
        source: u32,
        state: KeyState,
        emit: &mut impl FnMut(Output) -> bool,
    ) -> bool {
        let Some(index) = MAPPINGS.iter().position(|mapping| mapping.source == source) else {
            return false;
        };
        let mapping = MAPPINGS[index];
        let slot = &mut self.slots[index];

        match state {
            KeyState::Down => match slot.source {
                SourceState::Idle => {
                    if slot.output_down {
                        // 前回のキー解放に失敗しているため、対応する解放のない押下を追加しない。
                        slot.source = SourceState::Canceled;
                        return true;
                    }
                    if self.active_window.is_some() && emit(Output { mapping, state }) {
                        slot.source = SourceState::Remapped;
                        slot.output_down = true;
                        true
                    } else {
                        slot.source = SourceState::Forwarded;
                        false
                    }
                }
                SourceState::Forwarded => false,
                SourceState::Remapped => {
                    // 押下状態の記録を増やさずに、キーリピートを変換先へ送る。
                    emit(Output { mapping, state });
                    true
                }
                SourceState::Canceled => true,
            },
            KeyState::Up => {
                let suppress = matches!(slot.source, SourceState::Remapped | SourceState::Canceled);
                // 元キーの押下は終了するが、変換先の解放待ちは送信成功まで残す。
                slot.source = SourceState::Idle;
                if slot.output_down && emit(Output { mapping, state }) {
                    slot.output_down = false;
                }
                suppress
            }
        }
    }

    /// 元キーが解放済み、または変換を中止済みで、解放が完了していない出力に KeyUp を送る。
    /// 変換継続中の出力は解放せず、送信に成功したものだけ解放待ちの記録を消す。
    pub fn retry_releases(&mut self, emit: &mut impl FnMut(Output) -> bool) {
        for (mapping, slot) in MAPPINGS.iter().zip(&mut self.slots) {
            if slot.output_down
                && slot.source != SourceState::Remapped
                && emit(Output {
                    mapping: *mapping,
                    state: KeyState::Up,
                })
            {
                slot.output_down = false;
            }
        }
    }
}
