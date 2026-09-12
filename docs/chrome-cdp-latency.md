# Chrome JS発音 → Windows PCM取得の実測

2026-09-12、Chrome 152.0.7977.76、既定出力VB-Audio Virtual Cableのまま測定。
**Chrome内でAudioBufferSourceNode.start()を呼んでから、音の入ったPCMブロックを別プロセスが取得するまで、interactive設定で中央値72.497msだった。**

| AudioContext latencyHint | 検出 / 発音 | 最小 | 中央値 | 最大 | 時計合わせの推定誤差 |
|---|---:|---:|---:|---:|---:|
| interactive | 30 / 30 | 58.916ms | 72.497ms | 78.031ms | ±0.230ms |
| playback | 30 / 30 | 80.425ms | 89.352ms | 99.905ms | ±0.181ms |

30回ではnearest-rank p99が最大値と同じになるため、表では最大値を示す。運用上の最大遅延保証ではない。
ChromeのbaseLatency報告値はそれぞれ10ms / 20ms、outputLatencyは両方0だった。これらは今回の測定区間の総遅延を表さない。

## 測定方法

- ユーザー指定のCDP方式で、専用プロファイルのChromeをlocalhostのデバッグポートで起動。通常のChromeは再起動していない。
- Chrome内の48kHz Web Audioで、20msの1kHzテスト音を30回生成。振幅0.005、間隔610〜899ms。
- Chrome内のperformance.now()でstart()呼び出しの直前・直後を記録。外からCDP命令を送った時刻を開始時刻には使わない。
- Windows取得はChrome親PIDと子を含むprocess loopback。音声入りブロックのGetBuffer完了直後をQPCで記録。検出処理・JSON保存より前の時刻を使う。
- QPCヘルパー↔Node、Node↔Chromeをそれぞれ測定前後50往復ずつ校正。往復で挟んだ時刻からオフセット区間を求め、前後の区間が整合することを確認。
- 表の誤差は時計のオフセット推定幅。発音時刻の量子化を別途±0.1msと仮定し、start()呼び出し幅（最大約0.1ms）も各パルスの区間に含めた。一定オフセット・量子化幅の仮定に基づく見積もりで、外部計測器による精度保証ではない。
- 100ms以上の静音に続く閾値0.0001超えを検出し、直前400ms以内の発音と1対1で対応づけた。符号化した信号の相互相関ではないが、process loopbackでは両設定とも余剰検出・未検出0だった。

各設定32秒、process loopbackと既定エンドポイントloopbackを同時に記録した。
process loopbackの通知周期中央値は10.005ms / 10.014ms。テレメトリ欠落、不連続、タイムスタンプエラーはいずれも0。
**約10msの通知周期から、発音→取得も10msと推定することはできない。**

エンドポイント全体では各30発に対して60回検出し、30回が余剰だった。最初の対応信号の中央値は210.244ms / 217.469msだが、既存の音声処理・再ルーティングを含む可能性があり、Chrome単独の遅延として採用しない。重複の原因は未特定。

## この結果の範囲

今回の開始点はJSの発音指示なので、Chrome内部の生成・バッファ・Windows取得までを含む。
WASAPI ReleaseBufferでPCMをWindowsへ渡した瞬間を計測していないため、その境界以降だけの遅延は分離できない。
YouTube / YouTube Musicの動画・楽曲デコード経路そのものの測定でもない。
Chromeの設定をinteractiveにする効果は観測したが、試行は各1回であり、差の全量を単一のバッファに帰属させない。
ネットワーク送信、macOSバッファ、Core Audio、物理出音は対象外。

## 再現・保存物

Rustのreleaseビルド、Node.js（この試行は24.19.0）、Visual Studio C++ Build Toolsが必要。
Developer Command Promptからリポジトリ直下で実行する。

```bat
mkdir target\render-tap
cl /nologo /O2 /W4 /WX apps\render-tap\qpc-clock.cpp /Fo:target\render-tap\qpc-clock.obj /Fe:target\render-tap\qpc-clock.exe
node scripts\chrome-latency.mjs
```

- [測定データ・校正サンプル・生ログSHA256](results/2026-09-12-chrome-cdp.json)
- 元ログ: `runs/chrome-cdp-1789183826312/`。PCMは保存していない。
- 計測用Chrome、QPCヘルパー、取得プロセスは終了済み。デバッグポート9223の待受も終了を確認した。
- 集約JSONの各パルスの上下限は、実行後の集約で発音時刻の量子化±0.1msを追加した。中央値などの点推定は実行時の値から変更していない。
