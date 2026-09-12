# PCMを早い位置で取得する経路の調査

調査日: 2026-09-12。公開仕様の確認と既存計測ログの再確認。下記の新経路の実装・実測はまだ行っていない。

結論: 約59msはWindowsの固定下限ではない。ReleaseBuffer前のアプリ内PCM複製が、現在の再生待ちを回避する最も直接的な候補。
既存アプリ全体のミックスが必要なら、低周期の専用仮想出力デバイスが別の候補になる。
いずれも1–2msという保証を公開仕様からは導けない。

## 候補と条件

| 経路 | 早くできる理由・条件 | 残る制約 |
|---|---|---|
| アプリ内PCM複製 | WASAPIならGetBufferで得た領域にアプリがPCMを書いた後、ReleaseBuffer前に共有リングへコピーする。再生キューが消費されるまで待たない | アプリ改修、プラグイン、または対象プロセス内のAPIフックが必要。アプリの生成周期そのものは短縮しない |
| SFX APO | 音声エンジン内で各ストリームのミックス前にAPOProcessへPCMが渡される | アプリからエンジンが読むまでの待ちは残る。エンドポイントへの導入が必要。RAWではSFXを通らない |
| 専用仮想WaveRT出力 | 音声を物理Realtekへ送る代わりに自作出力へルーティングし、短い周期で送信側へ渡す設計 | ドライバ・インストール・出力先変更が必要。共有エンジン周期とアプリの先行バッファは残る。SysVADは完成した転送ドライバではない |
| WASAPI排他 / ASIO | 対応する音源アプリなら共有エンジンを通さず出力できる | 任意の他アプリの出力を取得するAPIではない。ASIOは音源側対応が必要。通常WASAPI loopbackは共有モード限定 |
| IAudioClient3 + 小さい先行バッファ | ドライバが許す小周期を音源側で要求し、必要以上に先行投入しない | このPCのRealtek共有最小480フレームでは1msにならない。受理した周期と実際の取得遅延を再測定する必要がある |

## アプリ内複製の具体案（仕様からの設計上の推論）

GetBufferで得たポインタ・フレーム数・音声形式を追跡し、ReleaseBufferの直前に固定容量共有リングへPCMとQPCをコピーする。
元のWASAPI呼び出しは通常どおり実行する。APIフックの実装基盤にはMicrosoft Detoursが候補だが、完成した音声キャプチャAPIではない。
SILENTフラグ、ReleaseBuffer失敗、形式変更、複数ストリーム、プロセス終了を扱う必要がある。
ネットワーク送信・メモリ確保・待機は音源のオーディオスレッドから分離する。

まず所有するテスト音源内で直接複製し、共有リングへの公開→別プロセス取得をQPCで測る。
その測定が成功してから対象アプリでフックを試す。異なるAPI、保護されたプロセス、アプリの互換性によって適用可否が異なる。
10ms分を一括生成するアプリから1ms周期の新しい音声が得られるわけではない。48フレームに小分けするだけでは生成遅延は減らない。
先行生成したPCMを早く送る場合、元アプリの映像・予定再生時刻との同期も別途維持する必要がある。

SFX/MFX/EFXのどこを選ぶかで取得内容が異なる。SFXは全アプリの最終ミックスではないため、handoffのmixed system PCM要件をそのまま満たさない。
複数アプリを出力API境界で取る場合も、各ストリームの時刻をそろえたミックス処理が必要になる。

## 約59msの解釈の補足

`runs/ks-cursor-final/source.jsonl` のsource_pulse 145件を再確認すると、padding_framesは初回0、以降576（48kHzで12ms）。
これは既にキューに入っている再生待ちフレーム数。約59msにはこの種の待ちも含まれる。
パルスのブロック内位置、エンジン、DSP、取得・検出条件などがあるため、単に59−12msを特定の段の遅延と断定しない。
固有符号による相関、ブロック内オフセット、描画clockを追加した再測定が必要。

## 一次資料

- [WASAPI GetBuffer](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiorenderclient-getbuffer): PCM領域の所有とReleaseBufferの順序。
- [GetCurrentPadding](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-getcurrentpadding): 再生待ちフレーム数。
- [Microsoft Detours](https://github.com/microsoft/Detours): プロセス内API計測・フックの基盤。
- [APO architecture](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/audio-processing-object-architecture): SFXはrenderのmix前、RAWではSFXを使用しない。
- [APO実装](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/implementing-audio-processing-objects): APOProcessとエンドポイントへの登録。
- [SysVAD](https://github.com/microsoft/Windows-driver-samples/tree/main/audio/sysvad): 仮想WaveRTドライバの開発例。
- [Low Latency Audio](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio): IAudioClient3、小バッファ、排他、ASIOの条件。
- [Loopback Recording](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording): shared限定、ハードウェアループバックピンを使用する場合がある。
- [Microsoft low-latency-audio](https://github.com/microsoft/low-latency-audio): UAC2/ASIO開発プロジェクト。閲覧時READMEは一般公開リリースなしと記載。Realtekの任意アプリ取得を即座に置き換えるAPIではない。
