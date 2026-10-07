# 開発ガイド

このファイルはリポジトリ全体に適用する。

## プロジェクト

Touhou WASD は、Windows 10 / Windows 11 向けの東方Project用キーリマップアプリ。
Rust（edition 2024、Rust 1.92 以降）と Slint を使用する。
仕様は `spec/spec.md`、利用方法とビルド手順は `README.md` を参照する。

## 構成と依存方向

- `src/main.rs`: アプリの起動。
- `src/control_panel/mod.rs`: Slint とリマップサービスの接続、起動エラーの表示。
- `src/control_panel/window.slint`: トグルボタンの GUI。
- `src/remapping/domain.rs`: 対象プロセス名、キー対応表、押下状態の管理。
- `src/remapping/windows.rs`: Win32 フック、アクティブウィンドウの検出、入力送信。
- `src/remapping/tests.rs`: リマップ規則と状態遷移のテスト。
- `tests/windows_input.rs`: 明示実行用の Windows デスクトップ統合テスト。
- `build.rs`: Slint ファイルのコンパイル。

ディレクトリは機能単位（package by feature）で分ける。
UI と Win32 アダプターはドメインの規則を利用し、ドメインは Slint や Win32 に依存させない。
この規模で不要な抽象化、DI コンテナー、インターフェース階層は追加しない。

## 実装上のルール

- コメントとドキュメントは日本語で書く。識別子や API 名は既存の命名に合わせる。
- 対象プロセス名は `spec/spec.md` の一覧に限定する。ASCII の大文字・小文字を区別せず、ファイル名の完全一致で判定する。
- キーの対応は W/A/S/D → 矢印、J → Z、K → X、L → 左 Shift とする。
- 起動時は OFF。通常のウィンドウ内には ON/OFF のトグルボタンだけを置く。
- KeyDown / KeyUp の両方、長押し、キーリピート、複数キーの同時押しを扱う。
- アプリ自身が生成した入力は再度リマップしない。変換対象外の入力はそのまま通す。
- OFF、フォーカス喪失、対象ウィンドウの切り替え、終了時には、生成したキーの押下状態を解放する。
- 変換を中止した物理キーは、離されるまで元の入力を抑制する。対象外で押し始めたキーは、離されるまで途中から変換しない。
- キーフックは専用スレッドで処理する。Win32 コールバックからパニックを外へ伝播させない。
- 入力はスキャンコードで送る。矢印の拡張キーフラグと左 Shift の区別を維持する。
- `unsafe` を使う箇所には、安全性の根拠を日本語のコメントで記載する。ハンドルとフックの解放を忘れない。
- 動作を変更したら、関連するテスト、仕様書、README の整合性を確認する。

## ビルド・実行

Windows の MSVC ツールチェーンと Visual Studio Build Tools の「C++ によるデスクトップ開発」を使用する。
MSVC のスタックサイズ設定は `.cargo/config.toml` にある。

```powershell
cargo run --locked
cargo build --release --locked
```

配布用の実行ファイルは `target/release/touhou-wasd.exe`。
依存関係を変更する場合は `Cargo.toml` と `Cargo.lock` を整合させる。

## 検証

コードを変更した場合は、変更内容に適したテストと以下のチェックを行う。
コメントやドキュメントだけの変更では、差分確認と必要な書式確認を行う。

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

通常の `cargo test` ではデスクトップ統合テストは実行されない。
GUI の起動・最小化・復元・終了を確認する場合:

```powershell
cargo test --locked --test windows_input -- --ignored --exact control_panel_starts_minimizes_restores_and_exits
```

フォーカス判定とキー変換を Windows の入力経路で確認する場合:

```powershell
cargo test --locked --test windows_input -- --ignored --exact remaps_only_foreground_targets_and_releases_held_keys
```

入力統合テストは検証用ウィンドウにフォーカスを移し、キー入力を送る。
対話可能なデスクトップで実行し、テスト中は他のウィンドウを操作しない。
Windows がフォーカス移動を拒否する環境ではタイムアウトするため、実行できなかった確認項目は報告する。
実ゲームでの確認と、検証用ウィンドウを使ったテストの結果は区別して報告する。
