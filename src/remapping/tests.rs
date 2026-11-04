use super::domain::{KeyState, MAPPINGS, Output, Remapper, is_target_process};

/// OS へ入力を送らず、送信成功を模擬してドメインが要求した出力だけを記録する。
#[derive(Default)]
struct RemappingHarness {
    remapper: Remapper,
    output_events: Vec<Output>,
}

impl RemappingHarness {
    fn context(&mut self, enabled: bool, target_window: Option<usize>) {
        self.remapper
            .set_context(enabled, target_window, &mut |output_event| {
                self.output_events.push(output_event);
                true
            });
    }

    fn key(&mut self, source_key: u8, key_state: KeyState) -> bool {
        self.remapper
            .handle(source_key as u32, key_state, &mut |output_event| {
                self.output_events.push(output_event);
                true
            })
    }
}

#[test]
fn process_allowlist_matches_every_specified_filename_case_insensitively() {
    for name in [
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
    ] {
        assert!(is_target_process(name), "missing target: {name}");
        assert!(is_target_process(&name.to_ascii_uppercase()));
        for similar in [
            format!("custom-{name}"),
            format!("{name}.bak"),
            format!(" {name}"),
            format!("{name} "),
            format!("C:\\games\\{name}"),
        ] {
            assert!(!is_target_process(&similar), "unexpected target: {similar}");
        }
    }
}

#[test]
fn process_allowlist_does_not_match_other_games_or_similarly_named_programs() {
    for name in [
        "th06.exe",
        "th6.exe",
        "th7.exe",
        "th8.exe",
        "th9.exe",
        "th9.5.exe",
        "th12.3.exe",
        "th12.5.exe",
        "th12.8.exe",
        "th14.3.exe",
        "th16.5.exe",
        "th18.5.exe",
        "th123.exe",
        "th5.exe",
        "th21.exe",
        "th9.1.exe",
        "th95.exe",
        "th006.exe",
        "th19.5.exe",
        "th06.exe.bak",
        "custom-th06.exe",
        "th06_custom.exe",
        "th06",
        "東方紅魔郷",
        "東方紅魔郷改.exe",
        "notepad.exe",
        "",
    ] {
        assert!(!is_target_process(name), "unexpected target: {name}");
    }
}

#[test]
fn specified_keys_have_distinct_physical_scan_codes_and_left_shift() {
    let expected_mappings = [
        (b'W', 0x48, true),
        (b'A', 0x4b, true),
        (b'S', 0x50, true),
        (b'D', 0x4d, true),
        (b'J', 0x2c, false),
        (b'K', 0x2d, false),
        (b'L', 0x2a, false),
    ];
    for (mapping, (source_key, scan_code, extended)) in MAPPINGS.iter().zip(expected_mappings) {
        assert_eq!(mapping.source_key, source_key as u32);
        assert_eq!(mapping.scan_code, scan_code);
        assert_eq!(mapping.extended, extended);
    }
}

#[test]
fn keydown_and_keyup_are_paired_for_every_mapping() {
    let mut harness = RemappingHarness::default();
    harness.context(true, Some(1));
    for mapping in MAPPINGS {
        assert!(harness.key(mapping.source_key as u8, KeyState::Down));
        assert!(harness.key(mapping.source_key as u8, KeyState::Up));
        assert_eq!(
            harness.output_events[harness.output_events.len() - 2..],
            [
                Output {
                    mapping,
                    key_state: KeyState::Down
                },
                Output {
                    mapping,
                    key_state: KeyState::Up
                },
            ]
        );
    }
}

#[test]
fn disabled_non_target_and_unmapped_inputs_pass_through() {
    let mut harness = RemappingHarness::default();
    for (enabled, target_window) in [(false, Some(1)), (true, None), (false, None)] {
        harness.context(enabled, target_window);
        assert!(!harness.key(b'W', KeyState::Down));
        assert!(!harness.key(b'W', KeyState::Up));
    }
    harness.context(true, Some(1));
    for unmapped_key in [b'Z', b'X', b'Q', 0x26, 0xa0] {
        assert!(!harness.key(unmapped_key, KeyState::Down));
        assert!(!harness.key(unmapped_key, KeyState::Up));
    }
    assert!(harness.output_events.is_empty());
}

#[test]
fn all_seven_keys_can_be_held_and_released_independently() {
    let mut harness = RemappingHarness::default();
    harness.context(true, Some(1));
    for mapping in MAPPINGS {
        assert!(harness.key(mapping.source_key as u8, KeyState::Down));
    }
    assert_eq!(harness.output_events.len(), 7);
    for mapping_index in [4, 0, 6, 3, 1, 5, 2] {
        assert!(harness.key(MAPPINGS[mapping_index].source_key as u8, KeyState::Up));
        assert_eq!(
            harness.output_events.last(),
            Some(&Output {
                mapping: MAPPINGS[mapping_index],
                key_state: KeyState::Up,
            })
        );
    }
    harness.context(false, None);
    assert_eq!(harness.output_events.len(), 14);
}

#[test]
fn holding_and_repeating_never_generates_an_early_or_duplicate_release() {
    let mut harness = RemappingHarness::default();
    harness.context(true, Some(1));
    for _ in 0..10 {
        assert!(harness.key(b'W', KeyState::Down));
        harness.context(true, Some(1));
    }
    assert_eq!(harness.output_events.len(), 10);
    assert!(
        harness
            .output_events
            .iter()
            .all(|output_event| output_event.key_state == KeyState::Down)
    );
    assert!(harness.key(b'W', KeyState::Up));
    harness.context(false, None);
    assert_eq!(harness.output_events.len(), 11);
    assert_eq!(
        harness.output_events.last().unwrap().key_state,
        KeyState::Up
    );
    assert!(!harness.key(b'W', KeyState::Up));
    assert_eq!(harness.output_events.len(), 11);
}

#[test]
fn focus_loss_disable_and_target_to_target_switch_release_all_held_outputs() {
    for (enabled, target_window) in [(true, None), (false, Some(1)), (true, Some(2))] {
        let mut harness = RemappingHarness::default();
        harness.context(true, Some(1));
        for mapping in MAPPINGS {
            assert!(harness.key(mapping.source_key as u8, KeyState::Down));
        }
        harness.context(enabled, target_window);
        assert_eq!(harness.output_events.len(), 14);
        assert!(
            harness.output_events[7..]
                .iter()
                .all(|output_event| output_event.key_state == KeyState::Up)
        );
        // 変換を中止した後も、物理キーが離されるまで元の入力を抑制する。
        harness.context(true, Some(1));
        for mapping in MAPPINGS {
            assert!(harness.key(mapping.source_key as u8, KeyState::Down));
            assert!(harness.key(mapping.source_key as u8, KeyState::Up));
        }
        assert_eq!(harness.output_events.len(), 14);
        assert!(harness.key(b'W', KeyState::Down));
        assert_eq!(harness.output_events.len(), 15);
    }
}

#[test]
fn a_press_started_outside_the_target_is_not_remapped_halfway_through() {
    let mut harness = RemappingHarness::default();
    assert!(!harness.key(b'J', KeyState::Down));
    harness.context(true, Some(1));
    assert!(!harness.key(b'J', KeyState::Down));
    assert!(!harness.key(b'J', KeyState::Up));
    assert!(harness.output_events.is_empty());
    assert!(harness.key(b'J', KeyState::Down));
    assert!(harness.key(b'J', KeyState::Up));
    assert_eq!(harness.output_events.len(), 2);
}

#[test]
fn keys_already_down_when_the_hook_starts_are_forwarded_until_released() {
    let mut harness = RemappingHarness::default();
    harness.remapper.mark_already_held(b'L' as u32);
    harness.context(true, Some(1));
    assert!(!harness.key(b'L', KeyState::Down));
    assert!(!harness.key(b'L', KeyState::Up));
    assert!(harness.output_events.is_empty());
    assert!(harness.key(b'L', KeyState::Down));
}

#[test]
fn failed_injection_forwards_the_entire_original_press() {
    let mut remapper = Remapper::default();
    remapper.set_context(true, Some(1), &mut |_| true);
    assert!(!remapper.handle(b'W' as u32, KeyState::Down, &mut |_| false));
    // 入力送信が復旧しても、押下途中で元の入力から変換先の入力へ切り替えない。
    assert!(
        !remapper.handle(b'W' as u32, KeyState::Down, &mut |_| panic!(
            "unexpected injection"
        ))
    );
    assert!(!remapper.handle(b'W' as u32, KeyState::Up, &mut |_| panic!(
        "unexpected injection"
    )));
    assert!(remapper.handle(b'W' as u32, KeyState::Down, &mut |_| true));
}

#[test]
fn failed_releases_are_retried_until_success_without_losing_ownership() {
    let mut remapper = Remapper::default();
    remapper.set_context(true, Some(1), &mut |_| true);
    assert!(remapper.handle(b'W' as u32, KeyState::Down, &mut |_| true));
    assert!(remapper.handle(b'W' as u32, KeyState::Up, &mut |_| false));
    let mut attempts = 0;
    remapper.retry_releases(&mut |output_event| {
        assert_eq!(output_event.key_state, KeyState::Up);
        attempts += 1;
        false
    });
    remapper.retry_releases(&mut |output_event| {
        assert_eq!(output_event.key_state, KeyState::Up);
        attempts += 1;
        true
    });
    remapper.retry_releases(&mut |_| panic!("already released"));
    assert_eq!(attempts, 2);
}

#[test]
fn failed_cancel_release_is_retried_while_the_physical_key_is_still_down() {
    let mut remapper = Remapper::default();
    remapper.set_context(true, Some(1), &mut |_| true);
    assert!(remapper.handle(b'L' as u32, KeyState::Down, &mut |_| true));
    remapper.set_context(false, None, &mut |_| false);
    remapper.set_context(false, None, &mut |output_event| {
        assert_eq!(output_event.mapping.scan_code, 0x2a);
        assert_eq!(output_event.key_state, KeyState::Up);
        true
    });
    assert!(remapper.handle(b'L' as u32, KeyState::Up, &mut |_| panic!(
        "already released"
    )));
}

#[cfg(windows)]
#[test]
fn windows_hook_service_starts_disabled_and_shuts_down_without_input() {
    let service = super::RemappingService::start().expect("install Win32 hooks");
    drop(service);
}
