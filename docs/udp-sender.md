# Chrome取得経路の採用とUDP送信

有線LAN条件での追試・packet単位の送信計測追加は
[Windows送信側のLAN遅延調査](windows-sender-latency.md)を参照。

2026-09-12。このPCの既定VB-Audio経路では、Chrome親PIDを指定する**process-includeを採用**する。
残る候補で改善を確認できなかったため、ユーザー指示に従ってUDP送信を実装し、Chromeテスト音→localhost受信を実測した。
これは現在の構成での採用判断であり、Windows／Chromeの理論的な最小遅延を確定したものではない。

## 最終取得比較

専用Chrome、interactive Web Audio、同じ30発を32秒間、各対応経路で同時取得。出力・音量設定は変更していない。
時計合わせの推定誤差±0.203ms。値はJS start()直前→PCMブロック取得。

| 経路 | 検出数 | 中央値 / 最大 (ms) | 判定 |
|---|---:|---:|---|
| process-include | 30 | 80.368 / 85.000 | 余剰・未検出0、採用 |
| process-exclude | 60 | 91.071 / 95.836 | 重複30。Chrome単独の遅延としては採用しない |
| 通常endpoint | 60 | 203.771 / 208.780 | 重複30 |
| PRE endpoint | 60 | 203.766 / 208.788 | 通常endpointからの改善なし、重複30 |
| IAudioClient3最小128 | 0 | — | 初期化失敗0x88890021 |
| POST endpoint | 0 | — | プロパティ設定失敗0x88890043 |

直接KSは追加試行でVB-Audio wave pin 1を開き、48フレーム通知／96フレームリングを受理した。
しかしChromeが30発を出した32秒間、通知0・取得PCM0。並行したprocess-includeは30/30検出した。
KSはこの設定で利用できず、通知が来ない理由を未特定のまま最小遅延値を付けない。
以前のRealtek直接KSは別出力・自作音源の試験で、リング周回による欠落追跡も未完成。今回の送信候補から除外する。
Chrome音声utilityへのDLL注入も以前の試行でロード失敗しており、今回再度セキュリティ設定を変更して試すことはしていない。

## 実装

- `--udp-to IP:PORT`で有効化。指定しなければ従来の診断のみ。Chrome用途は`--backend process-include --pid <親PID>`。
- GetBuffer直後、ReleaseBuffer前にPCMを事前確保したパケット領域へ直接コピー。
- 64個の固定プールと有界キュー。キャプチャでは割当・ファイルI/O・ソケット送信をしない。
- 既定128フレーム上限。480フレームを128+128+128+96へ即分割し、次のキャプチャを待たない。
- 1〜3送信ワーカー、既定1。MMCSS Pro Audio登録、非ブロッキングUDP。新着時にunparkする。
- 取得後5msを超えたパケットは送信APIを呼ぶ前に破棄。ネットワーク待ちで取得スレッドを止めない。OSによるスケジューリング・送信API内の経過時間まで5ms以内に保証する設定ではない。
- プール枯渇・期限切れ・送信失敗は再送せず計数。sequenceとfirst_sampleには穴を残す。
- 不連続・時刻エラーフラグ・有効なデバイス位置のジャンプでは音声sessionを更新し、隠れて時間軸を詰めない。process loopbackの未通知欠落は復元できない。
- f32 LEネイティブ形式のみ。無音フラグは明示的なゼロPCMへ。SRC／DSPは加えない。
- [72バイトのv1ヘッダーと共通goldenデータ](protocol.md)を定義した。macOS側はこの仕様を使って受信処理を追加する。

## localhostの測定

Chromeテスト音を各30発、process-includeで32秒取得し、UDPをNode受信器で検証した。
PCM本体は保存していない。送信側の時刻はQPC、受信Nodeの時計は前後100往復ずつ校正。
取得→送信は同じWindows QPC間なのでNodeの時計合わせの誤差を含まない。
下表はMMCSS対応後。単発の順次試験であり、厳密に負荷を隔離した優劣比較ではない。

| ワーカー | 送信 / 受信 | 取得→送信API直前 p50 / p99 / max (ms) | 取得→Node受信 p50 / p99 / max (ms) | 順序逆転 |
|---:|---:|---:|---:|---:|
| **1（既定）** | 12,792 / 12,792 | **0.0835 / 0.1776 / 0.2898** | 0.2153 / 2.0432 / 7.5178 | 0 |
| 2 | 12,792 / 12,792 | 0.0680 / 0.1438 / 0.2254 | 0.2096 / 1.3236 / 9.1435 | 1,856 |
| 3 | 12,796 / 12,796 | 0.0496 / 0.2069 / 0.6087 | 0.4350 / 5.1336 / 60.5111 | 3,028 |

全3試行でプール破棄・期限切れ・send失敗・受信欠落・重複・不正パケット0。
複数ワーカーでの順序逆転はsequenceで検出でき、並べ直すとsampleの穴は0。
最大データグラム1096バイト、フレーム数128または96。非ゼロPCMを受信し、ピーク0.005を確認した。
Node時計校正の推定誤差は順に±0.00965 / ±0.00990 / ±0.01290ms。

3ワーカーの受信側最大60.5msは、同じパケットの送信API直前→Nodeコールバックの区間に発生している。
受信OS／Nodeイベントループ側を含む区間で、NIC遅延や送信アプリの意図的なバッファと断定しない。
今回、1ワーカーで送信側p99が0.2ms未満かつ順序逆転0だったため、既定を1のままとする。

初回の通常優先度1ワーカーでは12,792発行中175パケットを期限切れで破棄した。
send呼び出し経過時間の最大18.946msも観測した。初回は開発作業の負荷も混在し、改善の全量がMMCSSだけによるとは断定しない。
初回の失敗も[集約結果](results/2026-09-12-udp.json)に保持している。

Chrome JS発音→PCM取得は、UDP付き3試行で中央値74.745 / 75.076 / 67.398ms。
送信部分が短くなっても、この上流遅延が消えるわけではない。ネットワーク送信とMac再生を含む総遅延は未測定。

## 再現・検証

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;" + $env:PATH
cargo build --release --locked
# QPC helperのビルドはchrome-cdp-latency.mdを参照
node scripts\udp-loopback-test.mjs

$env:CHROME_UDP_WORKERS = '2'  # または3
node scripts\udp-loopback-test.mjs

# endpoint/PRE/exclude/min/POSTの同一Chrome音源比較
$env:CHROME_FINAL_MATRIX = '1'
node scripts\chrome-latency.mjs
```

Chrome/CDP/QPCヘルパーの前提は[前の測定手順](chrome-cdp-latency.md)と同じ。
実送信用コマンド例（PIDと宛先を置き換える）:

```powershell
.\target\release\sender-windows.exe --backend process-include --pid 12345 --udp-to 192.168.1.20:40100 --udp-workers 1 --udp-deadline-ms 5 --seconds 60 --output runs\udp.jsonl
```

Rustテスト14件、Clippy警告なし、fmt確認、JS構文検証。送信APIへの成功はLAN上の到達保証ではない。
今後の工程はMac受信・sample位置による整列・欠落処理・Core Audio出力。今回はlocalhostまでで、LAN宛先への実送信はしていない。
