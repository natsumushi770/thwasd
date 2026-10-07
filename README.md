# Touhou WASD

Windows 10 / Windows 11 向けの東方Project用キーリマップアプリです。
Rust と Slint で実装しています。

## ビルド・実行

Rust 1.92 以降と Visual Studio Build Tools の「C++ によるデスクトップ開発」が必要です。
Windows の MSVC ツールチェーンで実行してください。

```powershell
cargo run --locked
```

配布用の実行ファイルを作成する場合:

```powershell
cargo build --release --locked
```

生成先は `target/release/touhou-wasd.exe` です。

## 使い方

起動時は **OFF** です。ウィンドウ内の唯一のトグルボタンで ON / OFF を切り替えます。
ON の間でも、対象ゲームがアクティブウィンドウになっている場合にだけ変換します。
最小化するとタスクバーに入り、そのまま動作します。閉じると終了します。

| 入力 | 変換先 |
| --- | --- |
| W | ↑ |
| A | ← |
| S | ↓ |
| D | → |
| J | Z |
| K | X |
| L | 左 Shift |

KeyDown / KeyUp、長押し、キーリピート、複数キーの同時押しに対応します。
対象外のキーと、このアプリ自身が生成した入力はそのまま通します。
ゲームからフォーカスを外す、OFF にする、アプリを終了する際には、変換先の押下状態を解放します。
その時点で押していた変換元キーは、一度離してから押し直すと再び変換されます。
対象外で押し始めたキーも、一度離すまでは元の入力として扱います。

対象プロセス名は `spec/spec.md` に記載した 23 個のファイル名です。
紅魔郷は `東方紅魔郷.exe`, `th06nc.exe`, `th06c.exe` が対象です。
`th06.exe` や `th123.exe`、`th9.5.exe` など一覧にない名前は対象になりません。
ファイル名は大文字・小文字を区別せず、完全一致で判定します。

ゲームを管理者として起動している場合は、このアプリも同じ権限で起動してください。
Windows の `SendInput` は、より高い権限で動作するアプリへの入力を送信できません。

## 構成

機能単位でディレクトリを分けています。

```text
src/
  main.rs                       アプリの起動
  control_panel/
    mod.rs                      Slint とサービスの接続・起動エラー表示
    window.slint                トグルボタンだけの GUI
  remapping/
    domain.rs                   対象名・対応表・キー押下状態のルール
    windows.rs                  Win32 キーフック・プロセス判定・入力送信
    tests.rs                    リマップ動作のテスト
```

ドメインは Slint / Win32 に依存しません。UI と Win32 アダプターが内側のルールを利用し、
この規模で不要なリポジトリ・DI コンテナー・インターフェース階層は設けていません。
Win32 フックは専用スレッドで処理し、対象プロセスの判定をフォーカス単位でキャッシュします。
入力はスキャンコードで送り、矢印には拡張キーフラグを指定します。

## 検証

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Windows 上で実アプリの起動・最小化・復元・終了を確認するテスト:

```powershell
cargo test --locked --test windows_input -- --ignored --exact control_panel_starts_minimizes_restores_and_exits
```

デスクトップ入力を確認するテスト（検証用ウィンドウにフォーカスを移してキーを送ります）:

```powershell
cargo test --locked --test windows_input -- --ignored --exact remaps_only_foreground_targets_and_releases_held_keys
```

この入力テストは対話可能なデスクトップで実行してください。Windows がフォーカス移動を
拒否する環境ではタイムアウトします。実行中は他のウィンドウを操作しないでください。
これらのデスクトップテストは通常の `cargo test` では実行しません。

実ゲームでは ON / OFF、斜め移動と射撃・低速移動の同時押し、長押し、
Alt+Tab 後のキー解放、最小化中の動作、終了後の通常入力を確認してください。

Win32 API の参考:
[LowLevelKeyboardProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc)、
[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)、
[KEYBDINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-keybdinput)。
GUI の参考: [Slint Rust API](https://docs.slint.dev/latest/docs/rust/slint/)。
