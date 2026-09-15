# macOS LAN Audio Sender

macOS 13以降向けの独立したAppKitアプリ。既存のLAN Audio ReceiverへLNAU v1 / Float32 stereoを送信します。

## ビルド・起動

```sh
scripts/build-macos-sender.sh
open 'dist/LAN Audio Sender.app'
```

Xcode Command Line Toolsが必要です。生成アプリはビルドしたMacのCPU向けです。
`SIGNING_IDENTITY`を指定しない場合はad-hoc署名です。

## 使い方

1. 「音声入力」でBlackHole、RMEなどを選択します。多チャンネル入力は隣接する2チャンネルを選択でき、モノラル入力は左右へ複製します。
2. 単一受信機なら送信先IPv4に受信MacのIPを入力します。マルチキャストなら既定の`239.255.0.1`を使い、受信側も同じグループに設定します。ポートは既定で`40100`です。
3. 複数NICでマルチキャスト送出経路を指定するときだけ「Multicast IF」に、この送信Macの該当NICのIPv4を入力します。空欄はOSの経路選択です。ユニキャストの送出経路はOSのルーティングに従います。
4. 受信側の周波数を入力デバイスと合わせ、「送信開始」を押します。初回はマイク権限、LAN送信では必要に応じローカルネットワーク権限を許可します。
5. 「送信停止」またはアプリ終了でキャプチャとソケットを閉じます。起動時に自動送信はしません。

Macの再生音は、再生アプリの出力をBlackHoleに設定し、本アプリの入力にもBlackHoleを選びます。アプリごとの出力選択がない場合はmacOSのサウンド出力をBlackHoleに設定します。直接のシステム音声キャプチャやプロセス別キャプチャはありません。

入力デバイス・送信先・ポート・Multicast IFは保存します。チャンネルは起動時に先頭ペアに戻ります。

## 音声と遅延

- AUHALの入力コールバックでデバイスの現在の周波数を使い、Float32 stereoに変換します。リサンプリングは行いません。
- デバイスのバッファサイズや周波数は変更しません。画面の入力framesが取得周期の目安です。例えば48 kHz / 512 framesは約10.7 msです。128 framesへのUDP分割だけで入力の待ち時間が短くなるわけではありません。
- UDPは最大128 frames、72-byteヘッダー込み最大1096 bytes。短い端数も直ちに送ります。
- コールバック内でメモリ確保をせず、nonblocking UDP sendを行います。送信失敗時は再送せず、sequenceとfirst_sampleを進めて欠落を保持します。厳密なリアルタイム保証や物理E2E遅延の測定結果はありません。
- 入力sample timeの不連続、入力失敗後の復帰でsessionを更新します。
- デバイスの抜き差しや周波数変更後は停止・再読込・再開してください。自動再接続はありません。
- 「送信中」はローカルのUDP送出を示し、受信機からの到達確認ではありません。

## 検証

```sh
cargo build --release --locked -p receiver-macos
scripts/build-macos-sender.sh
python3 scripts/test-macos-sender.py
cargo test --workspace --locked
'dist/LAN Audio Sender.app/Contents/MacOS/LAN Audio Sender' --list-devices
```

独立したPythonデコーダで分割・PCM・無音・timestamp・session更新・不正アドレスを確認し、Rust受信機への実UDP入力も検証します。

明示的な入力キャプチャ診断（音声再生なし、指定宛先へ送信）:

```sh
'dist/LAN Audio Sender.app/Contents/MacOS/LAN Audio Sender' --capture DEVICE_ID 127.0.0.1 PORT 3
```

2026-09-15: BlackHole 2ch / 48 kHz / 512 framesの3秒キャプチャで1,124パケット・143,872 framesを独立UDP受信し、連続sequence/sample・単一session、送信/入力エラー0を確認。CLIキャプチャとGUIではOSが権限を別に扱う場合があります。別Macへの到達、実際のスピーカー再生、長時間安定性は別途確認が必要です。

GUIでもマイク権限許可後にBlackHoleから127.0.0.1へ5,108パケットを連続受信し、開始・停止・入力選択の無効化/再有効化を確認しました。
