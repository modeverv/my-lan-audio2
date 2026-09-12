# YouTube Music / YouTube / Spotify 実アプリ取得試験

2026-09-12、このWindows PC。実際の再生ボタンを操作して確認した。
**SpotifyはWASAPI出力API入口から別プロセスへのPCM取得で、30秒試行の最大0.152msまで確認。**
Chromeの2対象は通常process loopbackで取得成功。出力APIフックはChrome音声プロセスへのDLLロードが拒否され、低遅延取得は未達。

追試で[Chrome内のJS発音→PCM取得](chrome-cdp-latency.md)を計測し、interactive設定で中央値72.497msを確認した。Web Audioのテスト音源による結果であり、以下のYouTube再生試験とは測定区間が異なる。

## 通常取得の実機確認

既定出力設定は変更していない（VB-Audio）。取得は指定プロセスとその子のprocess loopback。
Chromeの親PID17844、音声utility PID4172。Spotifyの音声APIをロードする親PID3280。
Chrome 152.0.7977.76、Spotifyパッケージ1.298.301.0 x64。

| 再生元 | 時間 | 取得ブロック / 非ゼロ音量 | 通知周期 p50 / p99 (ms) |
|---|---:|---:|---:|
| YouTube Music（既存Chromeアプリのページ） | 20秒 | 1,998 / 1,947 | 9.989 / 11.341 |
| YouTube（既存Chromeタブ） | 10秒 | 998 / 995 | 10.043 / 11.602 |
| Spotifyデスクトップ | 20秒 | 1,998 / 1,995 | 10.075 / 11.499 |

これらは音声取得の確認であり、実アプリの音源生成→取得の遅延ではない。
Chrome親ツリーの取得なので、タブごとの分離にはなっていない。
Spotify一時停止中、ChromeでYouTubeを再生した負の対照では、Spotify取得のピークは0、非ゼロブロック0。

YouTubeの新規自動作成タブは再生時間が進んでも20秒の取得がすべて無音だった。
既存タブで同じ動画を開くと取得できた。新規タブ側の消音・経路差は未特定であり、YouTube全般が取得不可という結果ではない。

## Spotifyの出力APIフック

[render-tap](../apps/render-tap/README.md)を新規実装。Microsoft Detours v4.0.1を使用。
Initialize / InitializeSharedAudioStream / GetServiceで実ストリーム形式を追跡し、
GetBufferのPCMをReleaseBuffer直前でコピー。元のReleaseBufferが返ると共有リングへ公開し、
別プロセスのrender-probe.exeが受信する。

| 試行 | 音声ブロック数 | API入口→別プロセス取得 p50 / p99 / max (ms) |
|---|---:|---:|
| 初回、受信中にログ出力 | 3,769 | 0.0175 / 0.9083 / 31.4610 |
| ログを測定後に出力 + 受信側MMCSS | 1,943 | 0.0176 / 0.0458 / 0.0988 |
| 同じ改良版、30秒再試験 | 2,994 | **0.0173 / 0.0444 / 0.1512** |

すべて取得したブロックで音声振幅が非ゼロ。ReleaseBuffer失敗、未知形式、リング満杯による破棄、producer競合は0。
最終試行のコピー完了までの時間は中央値1.3µs、p99 15.4µs、最大34.0µs。
44.1kHz / stereo / float32、通常441フレーム（10ms分）、最終試行の最大ブロックは882フレーム。
出力API呼び出し間隔は中央値9.9965ms、p99 11.6605ms、最大20.9807ms。

つまり、アプリが完成させた10ms分の音声ブロックを、Windowsの再生待ちを経由せずに短時間で取得できた。
アプリ内部のデコード・音声生成・先行バッファ時間を測ったわけではなく、毎1msで新しい音声が生成されるという結果でもない。
最初の31msの外れ値は受信ログI/Oやスケジューリングが候補だが、MMCSSとI/Oを同時変更したため単一原因の確定はしていない。
この短い試行での最大値は保証値ではない。ネットワーク送出・macOS再生・映像同期は未実装・未測定。

Spotifyを一時停止して同じフックを3秒動かす負の対照では、ReleaseBuffer呼び出し0・取得0だった。
その後の再開で上の30秒測定が成立した。

### 自作音源の予備試験についての訂正

最初の自作音源試験（中央値25µs、p99 665.5µs）は、PID 0モードの**同一プロセス内の別スレッド**だった。
作業中の説明で「別プロセス」と呼んだ部分は不正確。別プロセスへの実測値は上のSpotifyの3試行。

## Chromeの直接取得で止まった箇所

YouTube MusicとYouTubeは同じChromeの音声utility PID4172を使う構成だった。
対象を開き、対応するAudioSes.dllの関数アドレスを解決し、LoadLibraryWの遠隔呼び出しまで試したが、DLLロードの戻り値は0。
フックのインストール前に失敗した。元ログは `runs/apps-live/ytmusic-tap.log`。
ログ末尾の `: 0` はcollector側のGetLastErrorであり、Chrome側の失敗理由を表す有効なエラーコードではない。
サンドボックス付き音声プロセスであることはコマンドラインから確認したが、ロード拒否の具体的原因はまだ特定していない。
サンドボックス・署名ポリシー・Windows保護設定は変更していない。

Chromeの1ms級取得については、署名・サンドボックスと互換性のある導入方法、またはChrome側の明示的なPCM出力経路を別途検証する必要がある。
Spotifyで成功した数値をChromeにも適用してはいけない。

## 保存物・終了状態

- [集約結果JSON](results/2026-09-12-live-apps.json): 各試行のstartup/end/summaryと元JSONLのSHA256。
- `runs/apps-live/`: イベント単位の時刻・形式・振幅ログ。楽曲PCMは保存していない。
- 新規コード: `apps/render-tap/{shared.h,tap.cpp,probe.cpp}`、ビルド: `scripts/build-render-tap.cmd`。
- 試験終了後は再生を一時停止し、既存の比較用タブを元のYouTube Musicホームへ戻した。
- SpotifyのPCM取得は有限時間で終了。DLLと無効状態のフックはSpotifyプロセス内に残るため、完全に除去するにはSpotifyの再起動が必要。起動時設定・レジストリ・ドライバへの登録はない。

プロトタイプは固定256件の形式表、x64限定、同一PIDへの同時接続非対応などの制約がある。
長時間運用、終了処理の一般化、アプリ更新への追従、ストリームの生成破棄が多い場合の検証は今後必要。
