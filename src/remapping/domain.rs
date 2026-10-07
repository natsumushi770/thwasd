//! Win32 や UI に依存しないリマップ規則とキー押下状態の管理。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mapping {
    pub source: u32,
    pub scan_code: u16,
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

pub(super) fn is_target_process(name: &str) -> bool {
    // 指定されたファイル名だけを、大文字・小文字を区別せずに完全一致で判定する。
    TARGET_PROCESS_NAMES
        .iter()
        .any(|target| name.eq_ignore_ascii_case(target))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyState {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Output {
    pub mapping: Mapping,
    pub state: KeyState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SourceState {
    #[default]
    Idle,
    Forwarded,
    Remapped,
    // フォーカス喪失や無効化の後も、物理キーが離されるまで元の入力を抑制する。
    Canceled,
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    source: SourceState,
    output_down: bool,
}

#[derive(Default)]
pub(super) struct Remapper {
    active_window: Option<usize>,
    slots: [Slot; MAPPINGS.len()],
}

impl Remapper {
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

    // フック導入時に押下中のキーを記録し、すでに通過した KeyDown を途中から変換しない。
    pub fn mark_already_held(&mut self, source: u32) {
        if let Some(index) = MAPPINGS.iter().position(|mapping| mapping.source == source) {
            self.slots[index].source = SourceState::Forwarded;
        }
    }

    /// 元のキーイベントを抑制する必要がある場合は true を返す。
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
                slot.source = SourceState::Idle;
                if slot.output_down && emit(Output { mapping, state }) {
                    slot.output_down = false;
                }
                suppress
            }
        }
    }

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
