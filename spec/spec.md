# 概要
東方ProjectのSTGゲーム (東方紅魔郷など) をWASD入力での移動などを行うためにキーをリマップするアプリケーション

# 仕様
## 対象プラットフォーム
- Windows 10 / Windows 11

## 動作
- このアプリケーションはウィンドウアプリケーションである
  - ウィンドウにはリマップの on/off を切り替えるトグルボタンのみがある
  - 閉じるボタンを押すとアプリケーションが終了する
  - 最小化ボタンを押すとタスクバーに最小化される
- あらかじめ指定したプロセス名のプロセスがアクティブウィンドウになっているときのみキーのリマップを行う
  - 対象プロセス名は以下の一覧とし、大文字・小文字を区別せず、ファイル名の完全一致で判定する
    - 東方紅魔郷.exe
    - th07.exe
    - th08.exe
    - th09.exe
    - th095.exe
    - th10.exe
    - th11.exe
    - th12.exe
    - th125.exe
    - th128.exe
    - th13.exe
    - th14.exe
    - th143.exe
    - th15.exe
    - th16.exe
    - th165.exe
    - th17.exe
    - th18.exe
    - th185.exe
    - th19.exe
    - th20.exe
    - th06nc.exe
    - th06c.exe
- キーのリマップの対応
  - W: up arrow key
  - A: left arrow key
  - S: down arrow key
  - D: right arrow key
  - J: Z
  - K: X
  - L: Left Shift
- キーリマップの動作
  - KeyDown / KeyUp の両方を変換する。
  - キーを押し続けている間は、変換先キーも押下状態として扱う
  - 複数キーの同時押しに対応する
  - アプリ自身が生成したキー入力は再度リマップしない

# 開発環境
- プログラミング言語は Rust を用いる
- GUI ライブラリは Slint を用いる
- ディレクトリ構成には package by feature を用いる
- Clean Architectureの依存方向は守るが、この小規模アプリに不要な抽象化は行わない
