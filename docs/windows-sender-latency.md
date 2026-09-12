# Windows送信側のLAN遅延調査（2026-09-12）

## 結論と変更

現行有線LANの計測は、1ワーカーの処理能力不足説を**支持しない**。
既存GUIの約62分・約149万packetも、取得→send最大0.539ms、send区間最大0.506ms、
プール枯渇・期限切れ・WouldBlock・送信エラー0だった。
この既存ログは調査開始以前を含む観察データで、管理した比較試験とは分ける。

送信API以降→Mac到着の遅延原因と、Macの5msバッファでの欠落改善は未検証。
NIC・DPC・Mac受信のどれかと断定できない。今回の修正は観測精度の改善であり、
ネットワーク／再生レイテンシーを短縮したと主張するものではない。
ワーカー増加・NIC設定変更・期限延長は、採用根拠が得られていないため実施していない。

実装した対策：

- `send_call_us` の終点をAPI復帰直後へ移動。以前は送信後の結果処理・集計も含んでいた。
- `--events`でwire session/sequence/first_sampleと取得・公開・dequeue・park復帰・send開始/終了・結果を記録。
  期限切れ・プール枯渇等も記録し、受信できたpacketだけを見る偏りを避ける。
- 固定容量の既存キューでメタデータを渡し、通常優先度のreporterが保存。PCMは保存しない。
  満杯なら待たず、`telemetry_dropped`を加算する。
- キャプチャのwakeとGetBuffer開始時刻、ワーカーCPU、p99.9、1ms/5ms超を追加。
- Windows内の区間解析と、MacログとのID照合スクリプトを追加。64bit session IDの丸めを防止。

## 実行条件

- ブランチ `master`、基点コミット `aba88e50c0d209c6ee247e0323e425dd96b84203`。
  変更後はこのコミットに本作業の差分を加えたビルド。バイナリSHA256を集約JSONに保存。
- GUIが使っていたバイナリと基準測定用コピーのSHA256は一致：
  `75a7a58c6688cd1e0835df9a9275165d5bcf7ff8056691ac4141c3ca7bef328d`。
- `process-include --pid 17844`（既存Chrome親PID）、48kHz/f32/stereo、1ワーカー、期限5ms、最大128frames。
- Windows `192.168.11.29` → Mac `192.168.11.65:40100`。
  Realtek Gaming USB 2.5GbE Family Controller、リンク2.5Gbps、driver 11.19.602.2025。
  Wi-FiはDisconnected。NICの設定を変更していない。
- startupの参照endpointはスピーカー Realtek(R) Audio、既存mute=trueを保持。
  process loopbackは対象プロセスの取得で、参照endpoint名をChrome実描画先の確定情報とは扱わない。
- キャプチャ・送信ともMMCSS Pro Audio登録成功。
- 同じChromeの連続再生を取得、`--measure-level`で非ゼロを確認。
  曲の進行に伴う音声内容・無音は固定テスト音と同条件ではない。PCM内容は保存しない。
- 元GUIは許可を得て正常終了。ビルド・Rustテストは90秒の収録と重ねていない。
  コード調査や文書作成は並行しており、PC負荷を完全隔離した実験ではない。
- Macの実行設定・受信ログは取得できていない。受信バッファ5ms／追加遅延0msは引き継ぎ値で、
  今回の実行中設定を検証した値ではない。
- 試験後は元の`dist/LAN-Audio-Sender.exe`を再起動し、同じChrome PID 17844・宛先で送信再開。
  GUI内蔵の既存エンジンはそのまま。新しい計測機能は`target/release/sender-windows.exe`にビルド済み。

## 90秒比較

単位µs。各試行35,992packet送信。基準版は既存集計（1µs切捨て）、
追加版はメタデータ再集計（0.1µs）。全期間を含む。

| 区間 | 基準1 p50/p99/max | 基準2 p50/p99/max | 追加版1 p50/p99/p99.9/max | 追加版2 p50/p99/p99.9/max |
|---|---:|---:|---:|---:|
| 取得→send直前 | 77 / 189 / 402 | 80 / 190 / 615 | 81.5 / 194.4 / 282.7 / 390.8 | 78.8 / 189.7 / 325.6 / 435.9 |
| 公開→send直前 | 68 / 174 / 387 | 71 / 177 / 606 | 72.7 / 180.3 / 257.4 / 375.4 | 69.3 / 173 / 286.4 / 419.6 |
| send区間 | 5 / 102 / 304 | 5 / 104 / 583 | 4.9 / 101.6 / 191.5 / 308.8 | 5 / 101.8 / 185.7 / 390.1 |
| 取得間隔 | 10024 / 11616.3 / 11968.8 | 10052.8 / 11424.6 / 11938.4 | 10066 / 11407.8 / 11680.2 / 12094.5 | 10068.6 / 11404.6 / 11969 / 12360.6 |

API終点の修正と通常の試行間変動があるため、maxの低下を性能改善とは断定しない。
取得→sendのp99も同程度で、計測追加による明瞭な悪化／改善はない。
基準版の送信区間p99.9や起動後5秒除外値は元の集計から復元できないため未取得。
追加版の全期間／先頭5秒除外後の全分布は[集約JSON](results/2026-09-12-windows-sender-latency.json)に保存。

全4試行で取得480frames×8,998回、毎回1packetをdrain。
discontinuity、timestamp error、session更新、プール枯渇、期限切れ、WouldBlock、send error、テレメトリ破棄は0。
取得→send、公開→send、send区間は1ms超も5ms超も0（基準版も最大が1ms未満）。
process positionは全て無効であり、欠落ゼロの証明には使わない。
Macの欠落・順序逆転は未取得。

追加版のwake→最初のGetBuffer開始はp99 15.9 / 15.3µs、最大215.7 / 214.3µs。
GetBuffer所要時間はp99 21 / 20µs、最大54 / 197µs。
drainはp99 51 / 53µs、最大286 / 286µs。これらも1ms超0。
送信ワーカーCPUは1コア換算0.833% / 0.764%、キャプチャは約0.191% / 0.191%。
基準版のキャプチャCPUは0.104% / 0.226%、旧版は送信ワーカーCPUを記録しない。

時系列例（追加版1のsend最大、session `10620088052643856489`）：

| sequence | first_sample | 取得→send直前 µs | send所要 µs | 結果 |
|---:|---:|---:|---:|---|
| 15663 | 1879584 | 77.6 | 5.0 | sent |
| 15664 | 1879680 | 29.2 | 308.8 | sent |
| 15665 | 1879808 | 339.5 | 7.7 | sent |
| 15666 | 1879936 | 347.6 | 4.0 | sent |

15664のsend中に次packetが待つ挙動は確認できるが、後続も0.35ms程度でsendを開始し、
5ms期限を圧迫する規模ではない。個々のQPCと前後packetは集約JSONに残した。

## 10分連続試験

2026-09-12 14:55:21〜15:05:22 JST、実取得600.001秒、計測追加版で実施。
送信239,992packetと`udp`メタデータ239,992件が一致し、破棄・失敗・テレメトリ欠落はすべて0。
取得59,998回すべて480frames・1packet/wake、discontinuityとtimestamp errorも0。
送信ワーカーのMMCSS登録成功。

| 区間（µs） | p50 | p99 | p99.9 | 最大 | 1ms超 / 5ms超 |
|---|---:|---:|---:|---:|---:|
| 取得→公開 | 6.9 | 23.6 | 42.0 | 280.6 | 0 / 0 |
| 公開→dequeue | 72.1 | 175.3 | 276.0 | 476.1 | 0 / 0 |
| 公開→send | 72.2 | 175.4 | 276.1 | 476.4 | 0 / 0 |
| 取得→send | 81.3 | 191.2 | 296.1 | 486.3 | 0 / 0 |
| send区間 | 4.8 | 100.8 | 187.2 | 377.6 | 0 / 0 |
| 起床→最初のGetBuffer | 2.1 | 16.0 | 38.5 | 317.3 | 0 / 0 |
| GetBuffer | 5 | 20 | 25 | 181 | 0 / 0 |
| drain | 19 | 50 | 93 | 332 | 0 / 0 |
| 取得間隔 | 10073.5 | 11426.3 | 11680.9 | 11987.2 | 59997 / 59997 |

取得間隔は音声10ms分の生成周期を含む値なので、1ms/5ms超の全件を処理遅延異常と呼ばない。
15ms以上の取得間隔は観測していない。
CPUは1コア換算で送信ワーカー0.768%、キャプチャ0.211%、reporter等も含むプロセス全体2.497%。
音声ピークが閾値1e-6を超えた取得は59,164回。無音・微小音量の区間も除外せず集計した。
先頭5秒除外後の値と最悪packet前後も集約JSONに保存した。

**Windows送信内部に5msを使い切る事象はこの試験では再現しなかった。**
Macの到着・再生結果がないため、これをLAN損失ゼロ／Mac欠落解消とは解釈しない。

## 再現

同一宛先へ送るGUIを停止し、Chromeで音声を連続再生する。
PIDと宛先は実行時に確認する。CLIとGUIを同時送信すると別sessionが競合する。

```powershell
& "$env:USERPROFILE\.cargo\bin\cargo.exe" build --release --locked
.\scripts\benchmark-sender-latency.ps1 -SourcePid 17844 `
  -Destination 192.168.11.65:40100 -OutputDirectory runs\sender-next -Events
# 最終連続試験
.\scripts\benchmark-sender-latency.ps1 -SourcePid 17844 `
  -Destination 192.168.11.65:40100 -OutputDirectory runs\sender-long-next `
  -Seconds 600 -Repeats 1 -Events
```

実際の最初の比較では既存バイナリを `runs/sender-latency/baseline.exe`、
計測追加版を `instrumented.exe` に保存し、各90秒×2回を基準→基準→追加版→追加版で実行した。
引数は上記と同じで、基準版のみ`--events`なし。
管理スクリプトは新規ディレクトリを要求し、ログを上書きしない。
元ログ・バイナリはgit対象外の`runs/`、集約結果のみ`docs/results/`に保存する。

## Macとの同時収録・次の切り分け

MacのGUI受信器を停止してから、出力デバイスを確認し、同じ出力設定でCLIを起動する。
`--source`はWindows送信元IP、`--device`はデバイス名の部分一致または数値ID。
複数一致ならデバイス名を絞る。詳細は `receiver-macos --help` を参照。

```sh
./target/release/receiver-macos --source 192.168.11.29 \
  --device 'Fireface UCX II' --channel 1 --gain-db -12 --av-sync-delay-ms 0 \
  --buffer-ms 5 --buffer-frames 128 --network-scheduling realtime \
  --seconds 600 --events --output runs/mac-simultaneous.jsonl
```

出力1/2、48kHz、ASRC有効、追加遅延0ms、ゲイン−12dBも実際のstartupで確認する。
ASRCは既定有効（`--no-asrc`を指定しない）。
Windowsと重なる時間帯のログをWindowsへコピーした後：

```powershell
node scripts/analyze-sender-latency.mjs runs\sender-long-next\run-1.jsonl `
  runs\paired.json runs\mac-simultaneous.jsonl
```

`worst_arrival_excess`でMac到着間隔の拡大と同一packetのWindows send所要時間を照合する。
両機の時計を直接引き算しない。`sample_key_mismatches`とMacの順序逆転・重複も確認する。
収録窓から外れたsendを損失と誤認しない。

send区間が短いまま到着が遅れるなら、次は両端のpacket観測とWindows ETW/WPRの
CPU scheduling・DPC/ISR・ネットワークドライバを同時収録する。
packet captureは観測層を明記し、物理NIC送出時刻と同一視しない。
NICの省電力や割り込み集約を試すのは、該当区間の根拠を取得してから一項目ずつ。

## 仕様確認

Windows検証：`cargo build --release --locked`、workspaceのRustテスト28件、
`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo fmt --check`が成功。
Node解析テスト4件（64bit ID・順序逆転、分位点と閾値、起動区間の分離）も成功。
PowerShell再現スクリプトは構文解析後、実機の600秒試験に使用した。

[Microsoft send仕様](https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-send)では、
API成功は宛先での受信を保証しない。
[GetBuffer仕様](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer)の
QPC positionは100ns単位。本実装のQPCログも同じ単位に変換している。
process loopbackで無効なdevice positionから音声欠落を推定しない。
