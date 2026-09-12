# Windows送信QoS設定（2026-09-12）

## ドライバー更新後の調整（同日22:21）

Realtek公式ドライバー更新後、実ドライバーは `1156.23.20.605`（2026-06-05、oem75.inf）。
低遅延の検証用に、公開されている `*FlowControl` を3（受信・送信有効）から0（無効）へ変更した。
NIC再起動後の読み戻しで0、リンクUp・2.5Gbpsを確認。QoSのDSCP 46もActiveStoreで確認した。

フローコントロール無効化は、PAUSEフレームによるリンク全体の送信待ちを避けるための候補設定。
混雑時にはパケット損失が増える可能性があり、音声品質の改善を確認した設定ではない。
PAUSEフレームを実際に受けていたかも未測定。

更新後の確認値：

| 項目 | 最終値・扱い |
|---|---|
| フローコントロール | 無効へ変更 |
| EEE / Advanced EEE / Green Ethernet | 既に無効 |
| Idle Power Saving / Adaptive Link Speed / Gigabit Lite | 既に無効 |
| ジャンボフレーム | 既に無効 |
| 優先度およびVLAN | 有効を維持 |
| TCP/UDPチェックサムオフロード | 有効を維持 |
| LSO / RSC | 既存値を維持。今回の小さい送信UDPへの効果を確認していない |
| Transmit URBs / 伝送バッファ | 3 / 37を維持。単純な削減は行わない |
| Interrupt Moderation / Selective Suspend | 今回も公開プロパティなし |

NIC再接続後にComputer UseでGUIの停止・送信開始を実施し、エンジンPIDは6468から3172へ変更。
送信先と取得対象は維持。再開後のNIC統計ではSentBytes=13558774、送受信エラー・破棄は各0。
NIC再起動でカウンターがリセットされるため、以前の累積値と直接比較しない。
Mac到着・再生品質と実パケットのDSCPは未検証。

設定前後の保存先：`runs/realtek-low-latency/20260912-222129-780-*`。
フローコントロールだけ元に戻すには、管理者Windows PowerShellで以下を実行する。
NICが短時間再接続されるため、完了後にGUIの送信を停止・再開する。

```powershell
& 'D:\work\my-lan-audio2\scripts\set-realtek-low-latency.ps1' -RestoreFrom 'D:\work\my-lan-audio2\runs\realtek-low-latency\20260912-222129-780-restore.xml'
```

参考：PAUSEフレームによる送信停止の説明
https://www.intel.com/content/www/us/en/support/articles/000005593/ethernet-products.html
（Ethernetの仕組みの参考。Realtek固有の性能改善を保証するものではない。）

## 適用した設定

Windows 11 Proに永続的なローカルQoSポリシー `LAN Audio Sender UDP low latency` を作成した。

- 実行ファイル名: `sender-windows.exe`（GUIが起動する実エンジン）
- プロトコル: UDP
- 宛先ポート: 40100
- ネットワークプロファイル: All
- DSCP: 46（EF）
- 帯域制限、最小帯域割当、802.1p強制指定: なし
- 宛先IPは限定しない。現在のマルチキャストと、同ポートへのユニキャストを対象にする。

適用前のActiveStoreポリシーは0件。作成後のActiveStoreで上記条件を読み戻した。
保存先は `runs/network-qos/20260912-221031-353-*`。設定前のNICプロパティと統計も保存した。
最初の管理者起動は結果を残さず終了し、起動出力も保存する再実行で成功した。

## NIC確認（更新前の記録）

Realtek Gaming USB 2.5GbE Family Controller、driver 11.19.602.2025、有線リンク2.5Gbps。
QoSパケットスケジューラは有効。Priority & VLANは既にEnabled（値3）。

このドライバーの公開プロパティには、Interrupt Moderation、EEE、Green Ethernet、
Selective Suspendの調整項目がない。NetAdapterPowerManagementも対象オブジェクトを返さなかった。
デバイスマネージャーをComputer Useで確認したが、NICプロパティへの入力結果を確認できなかった。
非公開レジストリ値やUSB転送バッファ値を推測して変更する根拠はなく、NIC設定は変更していない。

## 確認範囲

Computer UseでGUIの停止・送信開始を操作し、エンジンPIDが20264から6952へ変わった。
取得対象はミックス済み音声、宛先は `239.255.0.1:40100` のまま。
GUIの「送信中」を確認。有線NICのSentBytesは316729324から322916568へ増加し、
OutboundPacketErrorsとOutboundDiscardedPacketsはともに0だった。
このカウンターはNIC全体であり、この音声ストリームだけの品質証明ではない。

実パケットのDSCPマーク、スイッチでの優先扱い、Macへの到着間隔、再生欠落は未測定。
設定が有効であることを確認した段階であり、遅延・ジッター改善は未確認。

## 復元

管理者のWindows PowerShellで以下を実行する。NICを変更していないため、復元対象は追加したQoSポリシーのみ。

```powershell
& 'D:\work\my-lan-audio2\scripts\set-sender-network-qos.ps1' -Restore
```

その後、GUIで送信を停止・再開する。スクリプトは同名ポリシーの条件が異なる場合は削除せず停止する。
更新後に変更したフローコントロールは、このQoS復元とは別に上記の方法で復元する。

## 参考

- https://learn.microsoft.com/en-us/powershell/module/netqos/new-netqospolicy
- https://learn.microsoft.com/en-us/windows-hardware/drivers/network/performance-in-network-adapters
