# macOS受信アプリ

2026-09-12。WindowsからMac/RMEへの実音声を受信・再生し、ユーザーの聴取確認まで完了。
受信実装は既存LNAU v1 wire形式と互換。以後Windows側にも追加の計測機能が実装された。

**受信スレッドの追加改善:** リアルタイム方式を既定にし、受信待ちp99を約1.9〜2.7msから約0.195msへ改善。[比較結果と制約](receiver-wakeup.md)。5msでの完全な安定動作はまだ未達。

## 起動

```sh
scripts/build-macos.sh
open "dist/LAN Audio Receiver.app"
```

Rust stable、Xcode Command Line Tools/Swiftが必要。エンジンはRust、CoreAudioへの小さいCブリッジ、GUIはAppKit。
アプリ内にエンジンを同梱し、ローカルでad-hoc署名する。外部配布向けのDeveloper ID署名／公証は未実施。

- Windowsの宛先: このMacでは **192.168.11.65 / UDP 40100**。
- Windows IP: 空欄なら、その起動で最初に受けた正しいstream 1の送信元IPへ固定。必要なら明示指定する。
- 出力: **Fireface UCX II (24100500)、再生チャンネル1/2**。
- この環境で採用した実用設定: **受信10ms、CoreAudio 16 frames（48kHzで約0.333ms）、映像同期追加0ms、ゲイン−12dB**。
- Windows HDMI → Epson EF21投影時、上記設定でユーザーが映像・音声の体感上の同期を確認（2026-09-12）。長時間ゼロ欠落や物理総遅延の測定とは別。
- 新規GUIの初期値は20ms／128frames。保存済み設定を優先する。実用設定を使う場合はGUIで選択する。
- 「受信開始」「停止」で制御。設定は次回へ保存される。受信中の設定変更には一旦停止が必要。
- 送信元のNIC/IPが切り替わったときは、Windows IP欄を空欄または新IPにして停止→開始する。
- ログは `~/Library/Logs/LAN-Audio-Receiver/`。PCMは保存せず、設定と数値のみをJSONL保存。
- CoreAudioサイズを明示設定した場合、通常停止時に起動前の値へ戻す。ただし他者がその設定を変更したと判断できる場合は上書きしない。SIGKILL／クラッシュ時の復元は保証しない。
- 出力デバイスの自動切替はしない。コールバックが停止した場合はエラー終了し、接続を確認して再開する。

CLI:

```sh
# 一覧（JSONL）
target/release/receiver-macos --list-devices

# 現在の有線LAN送信元を明示した60秒測定。出力JSONLの既存ファイルは上書きしない。
mkdir -p runs
target/release/receiver-macos --source 192.168.11.29 \
  --device 'Fireface UCX II' --channel 1 --buffer-frames 16 \
  --buffer-ms 10 --av-sync-delay-ms 0 --gain-db -12 \
  --seconds 60 --events --output runs/mac-example.jsonl

# 音声出力なしのネットワーク診断。dummy sinkは通常スレッドなのでCoreAudio評価には使わない。
target/release/receiver-macos --diagnostic --seconds 30

python3 scripts/analyze-receiver.py runs/mac-example.jsonl
```

`--seconds 0` は停止まで連続動作。CLIの `--buffer-frames 0` は既存設定を維持する。
`--no-asrc` はクロック微調整を無効化するが、送受信の公称レートが異なる場合のレート変換は残る。
GUIは受信2/5/10/20/40ms、CLIは2〜100ms。映像同期追加は0〜250ms。
CoreAudioのGUI選択肢は現在値維持、16/32/64/128/256/512frames。起動時の読戻し値と実callback framesをログに記録する。

## 最終の小バッファ試験と視聴確認

16frames／5msの90秒試験では35,912packet受信、実callback 16frames、
callback処理時間最大0.020167ms、間隔最大0.411667ms、callback discontinuity 0。
再生不足は2,531出力frames（合計52.729ms）、166callback、遅着26packet。
callback数はサイズによって変わるため、異なるサイズの比較では欠落frames・継続時間も見る。
[集約JSON](results/2026-09-12-16frames.json)。この試験だけで16framesの長時間安定を保証しない。

その後、ユーザーが16frames／10msでEF21へのHDMI投影とRME音声が体感上同期することを確認し、現段階の実用設定として採用した。
数値による両端比較は別条件（128frames／10ms）の[10分間同時計測](paired-lan-measurement.md)を参照。

## 実装

`UDP recvmsg → 事前確保256パケットキュー → sample-indexリング → ASRC → AUHAL callback`

- 72バイトLNAU v1を独立デコードし、golden datagramとPythonの独立エンコーダで互換を検証。
- 長さ、予約ビット、レート、形式、サンプル番号加算、有限floatを検証する。デコーダは1〜8chを扱うが現在の再生アプリはmono/stereoだけを受け付ける。monoは左右へ複製。
- 再生リングは131,072フレームの固定容量。各スロットにサンプル番号と世代を持ち、セッション切替時にPCM領域全体をクリアしない。
- sample番号で並べ、穴は無音にする。遅着は過去へ戻して再生せず、部分的遅着は残りだけを採用。重複・順序逆転・欠落を別々に計数。
- session IDで初期化し、既知のretired sessionと古いcapture QPCのセッションへ戻らない。最初のsession flagの到着には依存しない。
- 送信停止が500msを超えてから再開した場合は明示的にrebaseを計数し、新しい到着時刻から再開。500msを超える送信不在を延々アンダーランとして加算しない。
- 受信キューはスレッド分離用で、満たすまで待たない。コールバックごとに最大256個処理し、満杯時は新規投入を破棄・計数。時間待ちは単一のplayoutリングだけ。
- 最初の到着＋指定遅延でサンプルの再生を開始。起動遅れや大きなコールバック停止を追加バッファとして残さず、経過サンプル数を破棄・計数する。
- CoreAudio初期化後にbindする。初回実機試験で見つかった「初期化中のカーネル蓄積で約220ms余計に遅れる」問題を修正済み。
- SO_TIMESTAMPとIP_RECVIFでカーネル到着時刻／受信インターフェースを取得。再生期限はカーネル到着から計算し、スレッド起床待ちも見えるようにした。
- 受信スレッドは既定でMachリアルタイム・ポリシーを要求し、比較用にinteractive QoSを選択できる。コールバックにはsocket待ち、mutex、JSON、ファイルI/O、ヒープ割当を置かない。
- CoreAudioデバイス、出力チャンネルペア、実コールバックフレーム数・間隔・処理時間、出力timestampまでのleadを記録。

クロックは、送信サンプル進行／Mac到着時刻と、出力フレーム進行／Macコールバック時刻を**別々に**10秒窓で回帰する。
推定を20%ずつ平滑化し、相対補正は±1000ppm、変化は10ppm/秒に制限。公称レート比を含むASRC比率をログに出す。
受信ばらつきの影響を受ける推定であり、ppm値を物理水晶の精密測定とみなさない。

ASRCは差し替え可能な線形補間の初期実装。追加の長いフィルタ待ちはないが、帯域制限型の高品質SRCではない。
特に44.1/48kHz間などの大きなレート変換の音質保証は未実施。今回の実機は両側48kHz。

## 初期実測：有線LAN（CoreAudio 128frames）

WindowsをWi-Fiの192.168.11.28から有線LANの192.168.11.29へ変更したことをユーザーから確認。
Macの受信インターフェースはen8。変更後、同じ音声送信を各60秒ずつ順番に測定した。
開発作業の負荷は完全には隔離していない。全試行で不正パケット・アプリ受信キュー破棄・テレメトリ破棄0。
観測したsequence範囲にネットワーク未着パケットなし（先頭以前・末尾以後は評価できない）。

| 受信設定 | 受信パケット | 遅着packet | 欠落frame | アンダーランcallback | 到着間隔max | CoreAudio間隔max |
|---:|---:|---:|---:|---:|---:|---:|
| **20ms** | 23,912 | **0** | **0** | **0** | 22.917ms | 2.762ms |
| 10ms | 23,928 | 8 | 612 | 8 | 25.353ms | 2.740ms |
| 5ms | 23,916 | 64 | 5,748 | 61 | 31.341ms | 2.760ms |
| 2ms | 23,916 | 1,861 | 66,781 | 1,859 | 24.396ms | 2.751ms |

この初期試験から20msをGUI初期値にした。これは60秒試験の結果であり、長時間ゼロ欠落を保証するものではない。
10ms以下は実装済み・実測済みだが、この実測条件では安定合格にしない。
詳細な数値、receive→renderの区間、送信間隔とカーネル待ち時間は[集約JSON](results/2026-09-12-macos-wired.json)。

Wi-Fi時の90秒・20ms試験では35,924パケット、sequence未着0、遅着11packet、欠落1,218frame（25.375ms分）。
最大到着間隔42.692msの直前の送信間隔は9.064ms、そのパケットのカーネル内待ち0.771ms。
約33.6msの間隔増加は送信API呼び出し以降〜受信アプリまでの区間で、Mac再生コールバックの停止ではなかった。
それ以前のWi-Fi比較では100〜220ms級の外れ値も記録。NIC／無線／送信OSの各内訳までは分離していない。
[Wi-Fi時の集約](results/2026-09-12-macos-wifi.json)。pingはWindowsから応答なしで、UDP経路のRTT証拠には使用していない。

## 検証と残る範囲

```sh
cargo fmt --all -- --check
cargo test --release --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/test-macos-receiver.py
cargo run --release --locked -p receiver-core --example simulate -- --seconds 3600 --loss 0 --duplicate 0 --reorder 0
cargo run --release --locked -p receiver-core --example simulate -- --seconds 600 --loss 0.001 --duplicate 0.001 --reorder 0.01 --burst-ms 30 --stall-ms 20
```

- receiver-core 11テスト＋既存telemetry 2テスト。Windows固有テストはMacではコンパイル対象外。
- Pythonから実UDPへ独立エンコードし、不正6種、重複、順序逆転、session変更・retired session拒否を確認。
- 1時間の疑似ネットワーク（48kHz、Windows同様480-frameを4分割、±0.2ms jitter、sender +12ppm、receiver −5ppm、loss 0）は欠落0、深さ12.27〜27.65ms、有界。シミュレータのin-flight最大4packet。
- 10分の障害注入では226packetの意図的破棄、246重複、30ms burst、20ms callback stallを注入。欠落を計数し、59回のcallback停止を検出して古い音声を残さず復帰。深さ2.96〜25.98ms。
- GUI起動／受信開始／停止／再開／終了、実際のRME再生メーター、ユーザーの聴取確認を実施。
- EF21投影との同期はユーザーがテスト動画で目視・聴取確認済み。RMEで30分〜数時間の連続運用の定量評価、DACループバック、録画等によるプロジェクター同期の数値測定、高品質ASRC評価は未実施。
- Windows QPCとMac時刻は同期していない。**物理の端から端までの遅延／20ms未満の総遅延は未確定**。既存のChrome取得まで約67〜80msという上流実測は、Mac側実装で消えるものではない。
- Windowsを再起動してQPCがリセットされた場合はMacの受信を停止→開始する。ネットワーク認証・暗号化・複数送信元混合は対象外。
# IPv4マルチキャスト受信（2026-09-12追加）

Windowsの送信先が `239.255.0.1:40100` の場合、MacのGUIで次を指定します。

- UDPポート: `40100`（Windowsと同じ値）
- マルチキャストIP: `239.255.0.1`
- 受信LAN（MacのIPv4）: 受信するLANのMac自身のIP。今回の有線LANは `192.168.11.65`。空欄ならOSが選択します。
- Windows IP: 空欄で自動。指定する場合はWindows自身のIPで、グループIPではありません。

「受信開始」でグループへ参加し、停止時のソケット破棄で参加を解除します。Wi-Fiと有線LANが同時に有効なら受信LANを明示してください。ユニキャストへ戻す場合はマルチキャストIPと受信LANを両方空欄にします。設定は保存されます。

マルチキャスト有効時も同じポートへのユニキャストは受け取れます。既存の送信元固定・stream/session検証は継続します。対象はIPv4のLNAU v1 PCMです。

```sh
target/release/receiver-macos --multicast-group 239.255.0.1 \
  --multicast-interface 192.168.11.65 --bind 0.0.0.0:40100
# 音声出力なしのネットワーク検証
target/release/receiver-macos --diagnostic --seconds 10 \
  --multicast-group 239.255.0.1 --multicast-interface 192.168.11.65
# 別の一時ポートでユニキャスト／マルチキャストの実ソケット回帰試験
python3 scripts/test-multicast-macos.py
```

`--multicast-interface`の既定は`0.0.0.0`（OS選択）です。`--bind`は`0.0.0.0:PORT`のまま使い、グループIPやLAN IPをbind先にしないでください。起動ログに`multicast_group`と`multicast_interface`を記録し、参加失敗時には理由を表示します。

ローカル実ソケット試験で両方式30/30パケット、CLI診断でWindows `192.168.11.29`から有線LAN経由で10秒4,000パケット（invalid 0）を確認しました。ログ: `runs/multicast-live-20260912.jsonl`。

**このMacで残っている制約:** GUIから起動するとグループ参加には成功するものの受信0のままです。同梱エンジンのCLI診断では受信でき、GUIアプリがローカルネットワーク許可一覧に現れない状態を確認しました。GUI再生の検証は未完了です。受信開始時には、OSのLAN許可要求のためグループのUDP discardポートへconnectします（データ送信なし）。

[Apple TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)は、macOSのローカルネットワーク権限を安定して識別させるためApple発行の署名を推奨しています。現環境は有効な署名証明書0件・アドホック署名で、署名が原因かは未確定です。証明書がある環境では `SIGNING_IDENTITY='証明書名' scripts/build-macos.sh` で署名できます。権限ダイアログが出たらLANアクセスを許可してください。
