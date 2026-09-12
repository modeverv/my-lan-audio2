# Mac受信スレッドの起床待ち改善 — 2026-09-12

通常QoSの受信スレッドと、非周期のMachリアルタイムスケジューリングを同じ実行ファイルで比較した。
受信待ちのp99は約1.9〜2.7msから約0.195msへ改善し、リアルタイム版の合計71,844パケットでは1ms超が0件だった。
GUIに「低遅延（リアルタイム）／標準（比較用）」と、受信待ちのp99・最大値表示を追加。CLI/GUIとも低遅延方式を既定とする。

## 変更

- `--network-scheduling realtime|qos` で同一実装のスケジューリングだけを切り替えられる。
- 受信専用スレッドに `THREAD_TIME_CONSTRAINT_POLICY` を設定。
  `period=0`（外部から非周期に到着）、`computation=0.5ms`、`constraint=1ms`。
  ナノ秒をmach_timebase_infoの比率でMach絶対時間単位へ変換する。
- OSの設定／読み戻し結果と実値を `receiver_scheduling` に保存。今回のRT試行はすべてset/get成功、is_default=falseだった。
  設定が失敗した場合はinteractive QoSで続行し、ログとGUIの実動作表示で識別できる。
- `recvmsg` のブロッキング待ちを維持する。ビジーループ、sleepによる音声ポーリング、CoreAudioコールバック内のソケット処理は追加しない。
  100msのsocket timeoutは従来どおり停止確認用。
- PCMをメーター／ログ用情報の作成より先にキューへ公開。
- 受信スレッドの可変長retired-session HashSetを固定256要素へ変更。さらにcapture QPCの単調性チェックで古いセッションへの逆行を拒否する。
- キュー公開→CoreAudio取り出しの待ち時間を追加計測。既存の事前確保キューを維持し、根拠のないロックフリー構造の作り直しはしていない。

1msはスケジューラへの要求値であり、パケットの到着やOS全体に対するハードリアルタイム保証ではない。
`kernel_queue_ms` はカーネルtimestamp→recvmsg完了で、スケジューラのrun/wakeupトレースそのものではない。
そのため本文の「受信待ち」にはsocketキューやカーネル処理も含み、全量が純粋なCPU起床遅延とは断定しない。

Audio Workgroupsも検討した。非同期workerはデバイスの周期workgroupへ単純に加えるものではなく、独自intervalの設計が必要。
今回の外部到着待ちには非周期Machポリシーを適用し、まずその単独効果を確認した。独自Audio Workgroupは追加していない。
[Apple: Machスケジューリング](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/scheduler/scheduler.html) /
[Apple: 非同期Audio Workgroups](https://developer.apple.com/documentation/audiotoolbox/adding-asynchronous-real-time-threads-to-audio-workgroups)。

## 実機比較

Windows 192.168.11.29 → Mac en8 / 192.168.11.65、48kHzステレオ。
RME UCX II再生1/2、受信5ms、CoreAudio 128 frames、ASRC ON、ゲイン−12dB。
同じバイナリでQoS→RT→RT→QoSの順に各90秒。全パケットのメタデータログを使用し、下表は丸めたヒストグラムではなく生の値から再計算した。
送信音声は継続したが、OS／ネットワーク負荷を実験室のようには隔離していない。長時間保証や全構成への一般化はしない。

| 方式・試行 | 受信待ちp99 | p99.9 | 最大 | 1ms超packet | 5ms超packet | 欠落frame | underrun callback | CPU（1コア=100%） |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| QoS 1 | 1.927ms | 4.765ms | 9.352ms | 885 | 20 | 4,791 | 61 | 2.24% |
| RT 1 | 0.194ms | 0.228ms | 0.274ms | 0 | 0 | 3,576 | 39 | 2.36% |
| RT 2 | 0.195ms | 0.228ms | 0.368ms | 0 | 0 | 1,493 | 16 | 2.42% |
| QoS 2 | 2.714ms | 5.154ms | 7.874ms | 1,288 | 39 | 6,430 | 83 | 2.36% |

CPUは受信プロセス全体（JSONL生成を含む）のuser+system時間。Mac全体や受信スレッド単独のCPUではない。
不正パケット、受信キュー破棄、テレメトリ破棄、CoreAudio callback discontinuityは全4試行0。
最初の10秒を除いた集計でも傾向は同じ。生イベントから集約した[結果JSON](results/2026-09-12-receiver-wakeup.json)に保存。
それ以前の旧バイナリ90秒参考試行では最大20.991msを観測したが、最終比較表は同じ新バイナリ間に限定した。

## 残る遅延

RT化しても5ms設定の欠落はゼロになっていない。
RT 1の最大の到着間隔増加は14.130ms。そのパケットの送信間隔は10.351ms、Macの受信待ちは0.084msだった。
この例の主要な遅れは、受信スレッドより前の「送信API呼び出し以降→Mac到着」の区間にある。

キュー公開→CoreAudio取り出しはRTでもp99約2.65ms、最大約2.70ms。
これは128-frame callbackの約2.667ms周期による位相待ちと整合し、受信スレッドをRT化するだけでは消えない。
`handoff_batch_max_ms` はパケットのあるcallbackごとの最大値の分布で、全パケットの個別分布ではない。

今回の結論は **Macでの10ms級の受信待ちは大きく改善したが、5ms再生の安定化は未完了**。
次の対象は送信API以降〜Macカーネル到着までの揺らぎと、必要に応じた64-frame出力の比較。
受信バッファ20msを安定寄りの設定とする従来の判断を、今回だけで5ms安定へ更新しない。

## 再現・検証

GUIを停止してから、Windowsの送信を継続した状態で実行する。

```sh
scripts/build-macos.sh
python3 scripts/benchmark-receiver-wakeup.py --source 192.168.11.29 \
  --seconds 90 --buffer-ms 5 --frames 128 --output-dir runs/wakeup-repeat
python3 scripts/summarize-receiver-wakeup.py runs/wakeup-repeat

cargo test --release --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/test-macos-receiver.py --network-scheduling qos
python3 scripts/test-macos-receiver.py --network-scheduling realtime
```

receiver-core 11テスト＋既存telemetry 2テスト。実UDPの独立エンコード試験を両方式で通し、RT設定の読み戻しも検証。
GUIは受信方式の選択と実動作表示を持ち、標準方式へ戻せる。
