# macOS作業の開始点

Windows側の今回の範囲（Chrome取得、UDP送信、開始／停止GUI、localhost検証）は完了。
次はこのリポジトリでmacOS受信側を実装する。Windowsの取得方式を再調査する必要はない。

## まず読む資料

1. [元のハンドオフ](../LowLatency_Windows_to_macOS_Audio_Codex_HANDOFF.md): 全体設計。末尾§31がWindows実装の更新。
2. [UDPプロトコル](protocol.md)と[golden datagram](protocol-v1-golden.hex): 実装済みの正確なwire仕様。元資料の概念的なRust structよりこちらを優先。
3. [送信実装と測定結果](udp-sender.md): 取得方式の採用理由、遅延の区間、欠落・順序逆転の実測。
4. [Windows GUI](windows-gui.md): 接続・起動方法。

## 接続条件

- Windows: `dist/LAN-Audio-Sender.exe`を起動し、MacのLAN IPと受信ポートを入力、Chromeを選んで開始。
- 既定ポート40100。Mac側はUDPのbind先・ポートを設定可能にする。
- WindowsはChrome親PIDのprocess-include。今回の形式は48kHz、2ch、f32 LE。
- 480フレーム取得時は128/128/128/96フレームの4データグラムを即送信。1データグラム最大1400バイト。
- 既定1ワーカー、取得から5msを超えて未送信のパケットは破棄。受信確認のハンドシェイクはない。
- Windows GUIは設定保存・停止・再開に対応。連続送信は現在最大24時間。
- exeはWindowsのローカル成果物でGit対象外。Windowsで `scripts/build-gui.ps1` を実行すれば再生成できる。ソース・ビルド手順・集約実測データはGitに含む。

## macOSで実装する順序

1. UDP診断受信。goldenデータでデコードを検証し、実機でpacket数、形式、受信間隔、欠落、重複、順序逆転、session変更を計測する。
2. `(stream_id, session_id, first_sample)` による音声配置。固定容量の受信バッファをひとつだけ使い、sequence/sampleの穴を詰めない。古いsessionの遅着を破棄する。
3. 出力締切に基づく欠落補完・古いパケット破棄。複数ワーカーもあり得るので到着順を音声順にしない。
4. Core Audio → RME UCX IIへの出力。実デバイス形式・周期を計測し、コールバック内で割当／ネットワーク待ち／ログI/Oをしない。
5. クロックドリフト補正・ASRC、LAN越しの安定性と総遅延測定。

QPC時刻はWindows固有の単調時計。Macの時計とそのまま引き算しない。
first_sampleは取得したフレームの通し位置で、process loopbackの未通知欠落を復元できるハードウェア時計ではない。
既知の不連続ではWindowsがsessionを更新するため、受信側も音声時間軸を再初期化する。

## 完了済みの検証と残る制約

- Windows取得→UDP送信API直前: 1ワーカーの32秒試行でp50 0.0835ms、p99 0.1776ms、最大0.2898ms。
- 同試行12,792パケット全件localhost受信。GUIの開始／停止／再開試験でも合計29,292パケット全件受信。
- ChromeのJS発音指示→PCM取得は別区間で、試行中央値は概ね67〜80ms。通知周期10msをこの遅延と混同しない。
- Node診断受信にはスケジューリングの外れ値がある。Macの再生用受信器として流用する前提ではない。
- LAN越しの受信、Mac再生、映像同期、長時間のドリフトは未検証。Windowsの理論的な最小遅延や運用保証を確定したわけではない。
- 直接KS／DLLフックは実験コード。製品経路では使わない。Spotify用フックを通常GUIへ組み込んでいない。

Windows側の送信を停止している状態から作業を引き継ぐ。Mac受信器が準備できたらWindows GUIでMacのIPを指定して開始する。
