# WindowsのPRE/MUTE・アプリ別・直接KS取得の追加実験

実施: 2026-09-12、Windows 25H2 / 26200.9168、このPCのreleaseビルド。
通常WASAPIだけでなく、**既存RealtekドライバのKS/WaveRTループバックピンから直接PCMを読む経路**を実装した。
通知と音声読み取りの周期は約1msまで短縮できた。しかし、アプリがReleaseBufferで渡したパルスが取得側に現れるまで約59msあり、**音源から1msで取得する目標は未達**。

## 実装した経路

| 経路 | 実装とこのPCでの結果 |
|---|---|
| WASAPI PRE | IAudioClient2.SetClientProperties、Options NONE。出力を一時ミュートしても信号取得を確認 |
| WASAPI POST | POST_VOLUME_LOOPBACK (8) を試行。Realtek / VBとも0x88890043で非対応 |
| プロセス別 | ActivateAudioInterfaceAsyncのprocess loopback。指定PIDと子をinclude、または送信プロセスと子をexclude。ミュート中も取得 |
| KS能力調査 | IDeviceTopologyをたどり、IKsControlで実ドライバのピン・方向・AudioLoopback能力を取得 |
| 直接KS/WaveRT | KsCreatePin → 通知付き循環バッファ → ドライバイベント → PCM16 stereo 48kHzを読み取り |
| 独立音源の計測 | 別プロセスの440Hz・20msパルスを500msごとに送出。ReleaseBuffer前後のQPCと取得側検出QPCを記録 |

新しいカーネルドライバのインストールやアプリへのDLL注入は行っていない。process loopbackはWindowsの公開APIであり、アプリ内部のReleaseBufferをフックする実装ではない。
音量・ミュート前のタップと、音声エンジンのバッファ処理より前のタップは同じではない。

## 直接KSの実測

Realtek waveフィルタ `rearlineoutwavesst`、pin 3がloopback / DATAFLOW_OUT。
AudioLoopback能力は1（PREVOLUMEMUTE）。能力の取得結果は[Realtekピン一覧](results/ks-realtek-tree.json)に保存。
直接KS測定ではミュートを操作していないため、この経路の信号によるPRE検証は未実施。

`runs/ks-cursor-final`、各20秒、独立したRAW音源（通常共有初期化、480フレーム程度の描画周期）。
ビルド・テストは測定外で実行。実際の通知バッファサイズはドライバ応答を使用。

| 要求フレーム / 実通知区間 / 全リング | 通知間隔 p50 / p99 / max (ms) | 読み取った音声フレーム/回 | 音源Release→取得 p50 / p99 (ms) |
|---|---|---|---|
| 32 / 32 / 64 | 1.001 / 1.339 / 1.574 | 48 | 59.027 / 59.351 |
| 48 / 64 / 128 | 1.052 / 2.222 / 2.517 | 48 または96 | 59.133 / 59.453 |
| 128 / 128 / 256 | 2.923 / 3.336 / 3.550 | 平均128 | 59.246 / 60.316 |

要求48フレーム（1ms）をドライバは64フレームに丸めた。要求32では実際のカーソルがほぼ48フレームずつ進み、1ms分のPCMを約1msごとに読めた。
これは初期化の要求値だけでなく、ドライバ位置とPCMを実際に読んだ結果である。
キャプチャスレッドCPUは1コア換算でそれぞれ6.41%、7.19%、4.69%。テレメトリ欠落はすべて0。

### 欠落追跡の制約

RealtekはGETREADPACKETを0x80070490で拒否した。`--ks-position` 指定時だけAUDIO_POSITIONのカーソル方式に切り替える。
カーソル差から未読範囲を読み、リング境界で折り返す。初回カーソルは基準位置として読み飛ばす。
全周以上進んだ場合は、位置だけから失われた周回数を確定できない。したがって全試行で `audio_timeline_valid=false`。

32フレーム設定では全リングの時間（1.333ms）以上の通知間隔が217回、カーソル問い合わせ間隔では257回あった。
これらは欠落の可能性を表す数で、失われたフレーム数ではない。48/128設定ではこのカウンタは0だが、完全な欠落検出を保証しない。
フレーム観測数は順に949,056 / 960,000 / 959,997。開始待ち時間や初回基準設定を含むため、単純に20秒×48kとの差をすべて欠落とは数えない。

先頭サンプルのドライバQPCはカーソル方式では得られず、サンプル年齢はnull。
一般summaryの `discontinuities=0` や `complete_telemetry=true` は「音声欠落ゼロ」を意味しない。
現段階のKS経路は診断用であり、連続タイムラインが必要な送信バックエンドとしては未完成。

初期の `runs/ks-position-32` などでは半バッファだけを読む暫定実装が音声量を過少計上していた。
この問題を修正し、リングをまたいで48フレームを読むテストを追加した。上表は修正後の再測定だけを使用している。

## 通常・PRE・プロセス別の比較

Realtek、各20秒、全経路で通知周期は概ね10ms。各ケース40パルスを対応づけた。
表は独立音源のReleaseBuffer完了→取得側のパルスを含むブロック取得の中央値 / p99。

| 取得経路 | 通常音源 (ms) | RAW + IAudioClient3最小480の音源 (ms) |
|---|---|---|
| 通常endpoint | 75.130 / 75.452 | 65.261 / 65.732 |
| PRE | 81.137 / 81.675 | 67.161 / 67.421 |
| PRE、endpoint mute ON | 77.155 / 77.466 | 59.982 / 60.302 |
| 指定音源PID include、mute ON | 78.229 / 79.161 | 66.481 / 67.410 |
| 自身をexclude、mute ON | 77.975 / 79.036 | 71.414 / 73.201 |

これは順次実行の小規模比較。RAWと初期化APIの両方が変わり、開始位相も異なるため、差をRAWだけの効果とは断定できない。
少なくともPRE/processだけでは1ms取得にならず、KSの通知を短くしても音源からの約59msは残った。
WASAPIの先頭サンプル年齢（この比較ではp99およそ11–12ms）は、音源Release→取得の代用品にならない。

VB-Audioでは描画用IAudioClient3の128フレームを受理しても、キャプチャは480フレーム・約10msのままだった。
明示した描画48フレームは0x88890020で拒否。Realtekの共有最小周期は480フレーム。
VBのprocess-includeではパルス24個、中央値42.327ms、p99 43.516ms。
VB endpoint/excludeにはパルス重複があり、比較の遅延値として採用しない。

Computer UseでEffeTuneのAudio Settingsを確認すると、入力がDefault CABLE Output、出力がDefault VB-Audioスピーカーになっていた。
複数のエフェクトも有効だった。この経路が重複の原因とまでは特定していない。設定はCancelし、変更していない。
実験スクリプトによるendpoint muteは各試行終了時に元の状態へ復帰する。

## 遅延測定の定義と限界

- 音源側はReleaseBuffer前後にQPCを採り、取得側はパルスを含むブロックの取得QPCを使う。音源は取得とは別プロセス。
- 20msパルスを500msごとに発生させ、100ms以上の静かなサンプル列の後の閾値超過を検出する。音源の振幅は0.005。
- 直前の音源パルスとの時間差が250ms未満なら対応候補とし、同じ音源の二重使用を拒否する。
- パルスには固有IDを符号化していない。雑音・反響・500ms周期の取り違えを完全には排除できない。重複や未対応がある場合は `pairing_ambiguous=true`。falseも厳密な相関の証明ではない。
- PCM本体は保存せず、ピークと時刻だけを記録。ネットワーク・macOS・DACまでの遅延は未測定。

## 再現と結果ファイル

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;" + $env:PATH
cargo build --release --locked
$device = '{0.0.0.00000000}.{07dce926-d0c4-42c5-810f-6dd7c0408d89}'
.\target\release\sender-windows.exe --device $device --ks-inspect
# inspectorのadapter_idとloopback pinを指定。このPCのRealtekはpin 3。
$inspection = Get-Content docs\results\ks-realtek-tree.json -Raw | ConvertFrom-Json
$wave = $inspection.adapters | Where-Object { $_.adapter_id -match 'wavesst$' }
.\scripts\kernel-benchmark.ps1 -Device $device -Filter $wave.adapter_id -Pin 3 -Seconds 20
.\scripts\backend-matrix.ps1 -Device $device -Seconds 20
.\scripts\backend-matrix.ps1 -Device $device -Seconds 20 -MinimumRender -RawRender
```

デバイスIDはこのPC固有。他のPCでは `--list-devices` / `--ks-inspect` から選択する。
[集約結果JSON](results/2026-09-12-extended.json)にはstartup / capture_end / summary / source_end、パルス解析、元JSONLのSHA256を収録。
巨大なイベント単位の元ログはgit対象外の `runs/` に保存している。集約JSONは測定当時のレコードを保存し、後から書き換えない。

検証: `cargo fmt --check`、workspaceテスト8件、Clippy `-D warnings` が成功。
実機ではprocess include/exclude、PRE mute、POST非対応、KSの32/48/128設定を実行した。

## 次に必要な実装

アプリから約1msの遅延を目指すには、いま残っている音源からタップまでの待ち時間を切り分ける必要がある。
まず固有符号のパルス相関と描画padding/clockを用いて計測精度を上げ、対象アプリの出力API境界でPCMを直接複製する方式を比較する。
Windowsのprocess loopbackだけではその境界に届いていない。
KSを送信用に使う場合も、単調増加する位置・パケット番号を持つドライバ経路と、DMA読み取り中の上書き検出が必要。
このPCの既存ドライバにGETREADPACKET対応を追加できたわけではない。

## 参照した公開仕様

- [IKsControlによるオーディオプロパティ取得](https://learn.microsoft.com/en-us/windows/win32/coreaudio/using-the-ikscontrol-interface-to-access-audio-properties)
- [AudioLoopback PRE/POST能力](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/kspropsetid-audioloopback)
- [通知付きWaveRTバッファ](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/ksproperty-rtaudio-buffer-with-notification)
- [GETREADPACKET](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/ksproperty-rtaudio-getreadpacket)
- [AUDIO_POSITION](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/ksproperty-audio-position)
- [Microsoftのprocess loopbackサンプル](https://raw.githubusercontent.com/microsoft/Windows-classic-samples/main/Samples/ApplicationLoopback/cpp/LoopbackCapture.cpp)
