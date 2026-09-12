# Windows / Mac両端同時計測 — 2026-09-12

## 結論

10分間、Windows送信239,992packetとMac受信239,992packetをwire session/sequenceで全件照合。
未到着0、重複0、sample位置不一致0、到着順序逆転18件だった。
Windows内の送信待ち・send API・Macのrecvmsg待ちはすべて最大1ms未満。
一方、send API復帰後→Macカーネル受信の区間で、隣接packetの間隔が最大13.813ms拡大した。

アプリ内の送信処理能力不足では説明できない到着の揺らぎを確認した。
ただしWindows OS・USB NIC・共有LAN・Mac NIC/OSの内訳は未分離。
「共有LANそのものの限界」とはまだ結論できない。

## 条件

- Windows modev@192.168.11.29、D:\work\my-lan-audio2。
- commit d667570f6fb8bb5429237dcc938d9e9db539abdb。
- Chrome親PID 2820、process-include、48000Hz/f32/stereo、480frames取得→128/128/128/96に分割。
- 送信1ワーカー、期限5ms、MMCSS Pro Audio登録成功。
- Realtek Gaming USB 2.5GbE、リンク2.5Gbps、Wi-Fi disconnected。
- Mac 192.168.11.65:40100/en8、RME UCX II出力1/2、−12dB、ASRC有効。
- 受信バッファ10ms、CoreAudio128frames、リアルタイム受信、AV追加遅延0。
- Windows取得600.000584秒、Macは615秒収録し前後を包含。
- Windows終了時刻2026-09-12 17:25:59 JST。Chromeはユーザーが連続再生、音源内容は固定していない。
- 両端ともPCMを保存せず全packetメタデータを記録。大量のログ転送は収録終了後。
- 送信側capture_success=true、complete_telemetry=true。両端のtelemetry破棄0。

SSH直起動はsession 0で音声ゼロとなり、本計測から除外した。
次の対話セッション試行も約77秒で0xC000013Aにより外部終了したため除外。
最終試行は一時的なスケジュールタスクのInteractive principalを使い、
ログオン中のmodev（session 1）で非表示送信器を起動。非ゼロ音声を確認した。
終了後タスクを削除。計測用送受信器は終了し、GUIの10ms設定を変更していない。

## 区間の統計

全期間、packetログから再計算。単位ms。

| 区間 | p50 | p99 | p99.9 | 最大 |
|---|---:|---:|---:|---:|
| Windows取得→send直前 | 0.0792 | 0.1499 | 0.1936 | 0.4260 |
| Windows send API | 0.0056 | 0.0874 | 0.1107 | 0.3923 |
| Macカーネル受信→recvmsg完了 | 0.082 | 0.218 | 0.258 | 0.421 |
| send復帰間隔に対するカーネル到着間隔の増加 | 0.024342 | 0.137800 | 0.398484 | 13.812792 |

最後の行は絶対片道遅延ではなく、隣接packet間の遅延の変化。
同一sessionかつ到着順でもsequenceが連続する239,937組のみ集計。
1ms超95組、5ms超43組、10ms超10組。
負の変化も分布に含める。順序逆転周辺は除外するため全遅延事象の網羅数ではない。

取得間隔はp50 10.0232ms、p99 11.0401ms、最大11.8096ms。
取得59,998回はすべて480frames、discontinuity・timestamp error・送信期限切れ・プール枯渇・送信失敗0。
取得周期の10msを処理時間の10msと解釈しない。

## 最も大きい事象

wire session 11088859985078951822、sequence 126812、Mac収録開始から321.513秒：

- Windows取得→send直前：0.0145ms。
- Windows send API：0.0951ms。
- 前packetからのsend開始間隔：10.5107ms。
- Macカーネル到着間隔：24.415292ms。
- send復帰間隔との差：13.812792ms。
- Macカーネル受信後の待ち：0.165ms。

送信API内部もMacアプリの受信待ちも短いまま、到着までの間隔が伸びた例。
「そのpacketの片道遅延が13.8ms」とは主張しない。

## 再生への影響

最後のpacket到着直前のreceiver_summary（Mac相対604.391522秒）で、
遅着49packet、ゼロ補完3,870出力frames（合計80.625ms）、underrun49callback。
合計補完時間は単一の連続音切れ時間ではない。
最終packet到着604.436388秒より前の集計なので、末尾約45msの再生はこの集計に含まれない。
送信停止後の意図的な音声不足を数えないため、receiver_finalの欠落値は使わない。

音声packetはすべて到着しており、再生不足をネットワークのpacket損失と呼ばない。
受信キュー破棄・overflow・CoreAudio callback discontinuityも0だった。
10ms設定でも今回の共有経路では不足が残ることを確認したが、5ms/20msや専用LANとの比較は未実施。

## 計算と再現資料

Macのreceive_time_nsはrecvmsg後、kernel_queue_msはカーネル受信からの待ち。
隣接packet a,bについて：

```
kernel_interval_ms = (receive_b - receive_a) / 1e6 - (kernel_queue_b - kernel_queue_a)
send_end_interval_ms = (send_end_b - send_end_a) / 1e4
interval_expansion_ms = kernel_interval_ms - send_end_interval_ms
```

各機内の時刻差を使うので固定の時計オフセットは相殺される。
時計の周波数差・timestamp精度の影響は残り、物理NICのハードウェアtimestampではない。
取得→send・send APIの時間はWindows QPC同士で計算。

- [集約JSON・最悪20事象・生ログSHA256](results/2026-09-12-paired-lan.json)
- Macローカル生ログ：runs/paired-ssh/mac-hidden.jsonl、runs/paired-ssh/windows.jsonl。
- Windows生ログ：D:\work\my-lan-audio2\runs\paired-hidden-win.jsonl。
- 既存照合：node scripts/analyze-sender-latency.mjs runs/paired-ssh/windows.jsonl runs/paired-ssh/windows-analysis.json runs/paired-ssh/mac-hidden.jsonl
- 今回の区間再計算：Macローカルruns/paired-ssh/summarize.py。

次に切り分けるなら、同じNIC・同じ設定で直結／専用スイッチと共有経路を比較する。
差が小さければ両端USB・NIC・ドライバー処理を、差が大きければ共有経路の競合通信・キューを優先する。
両端packet観測やWindows ETWでさらに観測地点を増やせるが、今回そこまでは実施していない。
