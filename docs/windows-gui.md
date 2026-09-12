# Windows GUI

起動ファイル: `dist/LAN-Audio-Sender.exe`。このexeをダブルクリックする。
このWindows PCでGUIの実操作とlocalhostへのUDP送信を確認した。

1. Chrome／Chromeアプリ、またはSpotifyを起動して音声を再生する。
2. 送信先のIPアドレスとポートを入力する。既定ポートは40100。
3. 「取得するアプリ」からChrome／ChromeアプリまたはSpotifyを選択し、「送信開始」を押す。
4. 「停止」で送信を終了できる。設定を変えて再開できる。

送信先は受信側アプリに合わせる。通常の音楽プレーヤーへ直接送信する形式ではなく、
[本プロジェクトのUDP v1形式](protocol.md)に対応する受信側が必要。Mac受信・再生の実装は別工程。
GUIの「送信中」は取得処理が動作中という表示で、UDP受信先からの到達確認ではない。

## 挙動

- ChromeとSpotifyの親プロセスを自動検出し、process-includeでその子を含めて取得する。複数の親プロセスがある場合は選択する。
- ChromeとChromeアプリ（YouTube Music等）は同じ親プロセスを共有する場合、まとめて取得する。タブ／アプリごとの分離はしない。
- Spotifyは通常process loopback。実験用の超低遅延DLLフックは使用しない。
- PIDは固定保存しない。取得対象を再起動したら「再検索」する。
- IPアドレス（IPv4/IPv6）・ポートの検証。空欄のまま送信しない。
- 送信開始時にIP・ポートを保存し、次回起動時に復元する。自動送信はしない。
- 送信中は設定と開始ボタンを無効化し、二重起動を防ぐ。
- 停止では標準入力の制御メッセージでエンジンを正常終了し、キューの処理・集計を完了する。
- ウィンドウを閉じると送信も停止。終了応答が10秒ない場合は自分が起動した送信プロセスを終了する。
- GUIが異常終了してパイプが閉じた場合も、送信エンジンはEOFを検知して停止する。
- 連続送信は現在最大24時間。その後はGUIに終了を表示し、再度開始できる。
- 既定1ワーカー・5ms送信期限・128フレーム分割。GUI自体でPCM処理や再バッファリングをしない。

## 配布・設定

Windows x64用のWinForms GUI。.NET FrameworkのOS環境を利用する。このPCでは追加インストール不要だった。
Rustの送信エンジンをexeリソースとして同梱し、初回実行時にユーザー用フォルダへ展開する。
展開済みエンジンはSHA256を確認し、ビルドごとのフォルダで管理する。管理者権限・レジストリ登録・自動起動は不要。

- 設定: `%LOCALAPPDATA%\LanAudioSender\settings.xml`
- エンジン: `%LOCALAPPDATA%\LanAudioSender\engine\<SHA256>\sender-windows.exe`
- 診断ログ: `%LOCALAPPDATA%\LanAudioSender\logs\`

ログは時刻・取得／送信の統計のみで、PCMは保存しない。古いログは自動削除しない。
exeはコード署名を付けていない。別PCのランタイム・ドライバ・表示倍率の全組み合わせは未検証。

## ビルド・検証

```powershell
.\scripts\build-gui.ps1
```

Rust releaseエンジンを作成し、Windowsの.NET Framework C#コンパイラでWinExeとして同梱ビルドする。
出力は`dist/LAN-Audio-Sender.exe`。別のCLI exeを同じフォルダへコピーする必要はない。

Computer Useで表示、送信先未入力エラー、送信開始、停止、再開、送信中のウィンドウ終了を実操作確認。
localhost受信器で2回合計29,292データグラムを全件受信し、不正パケット0、送信エラー・期限切れ破棄0。
正常停止後にsender-windowsプロセスが残っていないこと、GUI再起動でIP・ポートが復元され停止状態になることを確認した。
[GUI試験の記録](results/2026-09-12-gui.json)に送信統計を保存した。
Rustテスト14件、fmt、Clippy、C#警告をエラーにするビルドが成功。
