# Windows → macOS low-latency network audio

**2026-09-12：現段階の実装を完了。** Windows → 有線LAN → Mac → RMEの音声と、HDMI → Epson EF21の投影映像について、ユーザーが体感上の同期を確認しました。[Macの起動手順・実測](docs/macos-receiver.md)。

```sh
scripts/build-macos.sh
open "dist/LAN Audio Receiver.app"
```

この環境での実用設定は次のとおりです。アプリメニューの「設定…」（⌘,）で設定して、メインウィンドウの「受信開始」を押してください。

メインウィンドウには受信開始／停止と、設定を開く歯車ボタンがあります。出力・ネットワーク・バッファ・ゲイン設定と受信状況・ログは設定ウィンドウにまとめています。設定はウィンドウを閉じると保存され、次の受信開始時に反映されます。受信中は設定を変更できません。

| 項目 | 設定 |
|---|---|
| Windows送信先 | 192.168.11.65:40100（MacのLAN IP） |
| Mac出力 | Fireface UCX II、再生1/2、48kHz |
| CoreAudioバッファ | **16 frames（約0.33ms）** |
| 受信バッファ | **10ms** |
| 映像同期の追加遅延 | **0ms** |
| 再生ゲイン | −12dB |
| 映像経路 | Windows HDMI → Epson EF21 |

これはこの機材での視聴確認に基づく設定です。新規起動のGUI初期値は従来の20ms／128framesで、保存済み設定があればそれを使います。
CoreAudioは「現在の設定を維持」または16/32/64/128/256/512framesから選べます。機器が対応する値を使用してください。
受信方式はリアルタイムを既定にし、GUIで標準QoSと切り替えられます。[受信待ち改善の実測](docs/receiver-wakeup.md)。
16frames／10msでEF21の映像と音声が合うという確認は、目視・聴取によるものです。長時間ゼロ欠落や音源から耳まで10msという意味ではありません。
5msは低遅延の試験設定で、16framesの90秒試験でも再生不足が残りました。音切れが気になる場合は受信バッファを20msへ増やして比較できます。

[両端同時計測](docs/paired-lan-measurement.md)では、10分間の239,992packetすべてが到着する一方、
送信API復帰後→Macカーネル到着の間隔が最大約13.8ms拡大しました。共有LAN・両端OS・NICの内訳は未分離です。
16frames／5msの[短時間試験結果](docs/results/2026-09-12-16frames.json)も保存しています。

## ダブルクリックで起動

Windowsが `239.255.0.1:40100` へ送信する場合も受信できます。Macの「マルチキャストIP」に `239.255.0.1`、「受信LAN」にMacの有線IPv4を入力してください。[設定手順](docs/macos-receiver.md)。

Windowsでビルドした `dist/LAN-Audio-Sender.exe` を起動してください。
送信先IP・ポートを入力し、「ミックス済み音声（全アプリ）」、Chrome／ChromeアプリまたはSpotifyを選択して「送信開始」。停止は「停止」ボタンです。
起動中の対象アプリを自動検索します。ChromeとChromeアプリは同じ親プロセス配下をまとめて取得します。送信エンジンはexe内に同梱済みです。[GUIの詳細](docs/windows-gui.md)
exeはGit対象外のローカル成果物です。新しいWindowsチェックアウトでは `scripts/build-gui.ps1` で生成してください。

現在の実装は **Windowsキャプチャ診断・UDP PCM送信とmacOS CoreAudio受信再生** です。
Chromeの取得にはprocess-includeを採用し、有線LANでの両端同時計測・RME再生まで実測しました。
PCMはファイルに保存しません。`--udp-to`を指定した場合だけネットワーク送信します。

```powershell
# PIDと宛先は使用環境に置き換える。Mac側で受信アプリを起動しておく。
.\target\release\sender-windows.exe --backend process-include --pid 12345 --udp-to 192.168.1.20:40100 --seconds 60 --output runs\udp.jsonl

# 専用Chromeのテスト音 → UDP → localhost受信を自動測定
node scripts\udp-loopback-test.mjs
```

送信は既定1ワーカー、期限5ms、1パケット最大128フレーム。480フレームを取得したら即座に4分割します。
[UDP実装と実測](docs/udp-sender.md) / [バイナリプロトコル](docs/protocol.md)。

追加比較では、**Realtekのカーネルドライバ直結で約1ms間隔のPCM読み取り**まで確認しています。
ただし、これはアプリの音声出力から1msで到着したという意味ではありません。
小さな循環バッファの周回・欠落追跡も未完成です。[追加実験と制約](docs/low-latency-backends.md)を参照してください。

YouTube Music / YouTube / Spotifyの実アプリ比較と、Spotifyの出力API直前からの
共有メモリ取得の試作は[実アプリの測定結果](docs/live-app-capture.md)を参照してください。

ChromeのJS発音からprocess loopback取得までの実測は、interactive設定で中央値72.497msでした。
[CDPによる測定方法と結果](docs/chrome-cdp-latency.md)を参照してください。

## Windowsでビルド・実行

Rust stable MSVC toolchainとVisual Studio C++ Build Tools / Windows SDKが必要です。
このPCではRustはインストール済みですがPATHにないため、必要なら以下を実行してください。

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;" + $env:PATH
cargo build --release --locked
.\target\release\sender-windows.exe --list-devices
New-Item -ItemType Directory -Force runs | Out-Null
.\target\release\sender-windows.exe --backend auto --seconds 180 --output runs\capture.jsonl
```

通常は測定したいアプリの音声を再生しながら実行します。`--test-tone` を付けると、
選択出力へ小音量の440Hz正弦波（-46 dBFS）を生成します。音量設定の変更は行いません。
既定は60秒、最大86400秒、Ctrl+Cで終了して最終集計を書き出します。
出力ファイルは上書きしません。新しい名前を使ってください。

```powershell
# 固定したデバイスとテスト信号で、legacy / IAudioClient3試行を順に比較
.\scripts\capture-benchmark.ps1 -Seconds 180 -TestTone

# 既定以外を選ぶ場合は --list-devices のIDを渡す
.\target\release\sender-windows.exe --device '{0.0.0.00000000}.{...}' --test-tone --seconds 180

# JSONLから最終集計を読み出す
.\scripts\summarize-run.ps1 -Path runs\capture.jsonl
```

`--backend legacy` は通常の共有モード、`min` はIAudioClient3最小周期の厳密な試行です。
`auto` は最小周期を試して失敗理由を記録し、新しいIAudioClientを生成してlegacyへ戻ります。
APIが要求を受理しても、実際のイベント周期が同じとは限りません。計測値を確認してください。

## 追加した取得経路

```powershell
# 送信プロセス自身とその子以外の音声を、出力デバイスに束縛されず取得
.\target\release\sender-windows.exe --backend process-exclude --seconds 30 --measure-level --output runs\process.jsonl

# 特定アプリとその子の音声を取得（PIDを指定）
.\target\release\sender-windows.exe --backend process-include --pid 1234 --seconds 30 --measure-level

# ミュート前の経路を試し、測定中だけミュートする。終了時に元の状態へ戻す
.\target\release\sender-windows.exe --backend legacy --tap pre --endpoint-mute on --measure-level --seconds 10

# カーネルドライバのピンとPRE/POST能力を読み取り
.\target\release\sender-windows.exe --ks-inspect

# 表示されたloopback出力ピンを指定して直接WaveRTを試す
.\target\release\sender-windows.exe --backend ks --ks-filter '<adapter_id>' --ks-pin 3 --ks-frames 32 --ks-position --measure-level --seconds 30 --output runs\ks.jsonl

# 別プロセスの同じパルス音源を使った通常/PRE/ミュート/アプリ別比較
.\scripts\backend-matrix.ps1 -Device '<endpoint_id>' -Seconds 20
.\scripts\backend-matrix.ps1 -Device '<endpoint_id>' -Seconds 20 -MinimumRender -RawRender
```

`--tap pre` は `SetClientProperties` のオプションNONE、`post` はPOST_VOLUME_LOOPBACKです。
実際のPRE対応は信号の振幅とKS能力の応答で検証します。音量変更を希望しない通常実行では
`--endpoint-mute keep`（既定）を使います。比較スクリプトはミュートON/OFFを一時変更します。
`--ks-position` はGETREADPACKET非対応ドライバに対する診断用のカーソル方式です。
全周回を識別できないので、ネットワーク送信のマスタータイムラインとしてはまだ使用できません。

**このPCの既定出力はVB-Audio Virtual Cableでした。** このアプリ自身はVoicemeeterや仮想ケーブルを
必要としません。物理Realtekも明示選択して計測できます。既定の出力先は自動で変更しません。
エンドポイントループバックは選択した出力へ流れるミックスを捕捉し、別の出力へルーティングされたアプリ音声は含みません。

## 計測項目

- デバイス名、ネイティブ形式、既定/最小デバイス周期、IAudioClient3共有エンジン周期、実際の設定周期
- イベント間隔のp50/p95/p99/maxとヒストグラム、イベントあたり/パケットあたりフレーム数
- QPCから求める先頭サンプル経過時間、GetBuffer時間、取得→診断レコード作成時間
- 不連続、無音フラグ、時刻エラー、デバイス位置の欠落/逆行、テレメトリ欠落
- 100ms/500ms以上のイベント間隔、500ms以上の音声量を持つパケット/イベント
- キャプチャスレッドのCPU時間、開始から最初のイベントまでの時間

イベント間隔・音声ブロック長・先頭サンプル経過時間は別物です。
ネットワークとmacOSの区間計測は実施済みです。DACを含む物理的な端から端までの総遅延は未測定です。

## 検証

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

[実機結果](docs/experiments.md) / [構造と次の工程](docs/architecture.md) /
[テレメトリ定義](docs/telemetry.md) / [プロトコル状態](docs/protocol.md)
