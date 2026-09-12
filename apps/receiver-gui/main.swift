import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var process: Process?
    var terminating = false
    var pending = Data()
    let devices = NSPopUpButton()
    let buffer = NSPopUpButton()
    let hardwareBuffer = NSPopUpButton()
    let scheduling = NSPopUpButton()
    let source = NSTextField(string: UserDefaults.standard.string(forKey: "source") ?? "")
    let port = NSTextField(string: UserDefaults.standard.string(forKey: "port") ?? "40100")
    let channel = NSTextField(string: UserDefaults.standard.string(forKey: "channel") ?? "1")
    let delay = NSTextField(string: UserDefaults.standard.string(forKey: "delay") ?? "0")
    let gain = NSTextField(string: UserDefaults.standard.string(forKey: "gain") ?? "-12")
    let start = NSButton(title: "受信開始", target: nil, action: nil)
    let stop = NSButton(title: "停止", target: nil, action: nil)
    let status = NSTextField(labelWithString: "停止中")
    let details = NSTextField(wrappingLabelWithString: "Windowsの送信先に、このMacのLAN IPとUDPポートを指定してください。\n受信開始で、選択したCoreAudioデバイスへ再生します。")
    let metrics = NSTextField(wrappingLabelWithString: "受信待機")
    let logLabel = NSTextField(wrappingLabelWithString: "ログ: 未開始")
    var logURL: URL?
    var engine: URL { Bundle.main.url(forResource: "receiver-macos", withExtension: nil)! }
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        if let iconURL = Bundle.main.url(forResource: "AppIcon", withExtension: "icns"),
           let icon = NSImage(contentsOf: iconURL) {
            NSApp.applicationIconImage = icon
        }
        let menu = NSMenu()
        let appMenu = NSMenuItem(); menu.addItem(appMenu)
        let submenu = NSMenu(); submenu.addItem(withTitle: "LAN Audio Receiverを終了", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q");appMenu.submenu = submenu
        NSApp.mainMenu = menu
        window = NSWindow(contentRect: NSRect(x: 0,y: 0,width: 670,height: 710),styleMask: [.titled,.closable,.miniaturizable],backing: .buffered,defer: false)
        window.title = "LAN Audio Receiver"
        let stack = NSStackView();stack.orientation = .vertical;stack.alignment = .leading;stack.spacing = 15
        stack.translatesAutoresizingMaskIntoConstraints = false
        window.contentView!.addSubview(stack)
        NSLayoutConstraint.activate([stack.leadingAnchor.constraint(equalTo:window.contentView!.leadingAnchor,constant:24),stack.trailingAnchor.constraint(equalTo:window.contentView!.trailingAnchor,constant:-24),stack.topAnchor.constraint(equalTo:window.contentView!.topAnchor,constant:22)])
        let title = NSTextField(labelWithString: "Windows → Mac Audio");title.font = .systemFont(ofSize:25,weight:.semibold);stack.addArrangedSubview(title)
        status.font = .systemFont(ofSize:15,weight:.medium);stack.addArrangedSubview(status)
        details.font = .systemFont(ofSize:12);details.textColor = .secondaryLabelColor;stack.addArrangedSubview(details)
        func row(_ label:String,_ view:NSView) {let text = NSTextField(labelWithString:label);text.widthAnchor.constraint(equalToConstant:180).isActive = true;view.widthAnchor.constraint(equalToConstant:395).isActive = true;let r = NSStackView(views:[text,view]);r.spacing = 12;stack.addArrangedSubview(r)}
        loadDevices()
        row("出力デバイス",devices)
        let refresh = NSButton(title:"デバイス一覧を更新",target:self,action:#selector(refreshDevices));stack.addArrangedSubview(refresh)
        row("Windows IP（空欄で自動）",source);row("UDPポート",port);row("出力チャンネル先頭（1始まり）",channel)
        buffer.addItems(withTitles:["2 ms","5 ms","10 ms","20 ms","40 ms"]);buffer.selectItem(withTitle:UserDefaults.standard.string(forKey:"buffer") ?? "20 ms")
        hardwareBuffer.addItems(withTitles:["現在の設定を維持", "16 frames", "32 frames", "64 frames", "128 frames", "256 frames", "512 frames"]);hardwareBuffer.selectItem(withTitle:UserDefaults.standard.string(forKey:"hardwareBuffer") ?? "128 frames")
        row("CoreAudioバッファ（停止時に復元）",hardwareBuffer)
        scheduling.addItems(withTitles:["低遅延（リアルタイム）", "標準（比較用）"])
        scheduling.selectItem(at:UserDefaults.standard.string(forKey:"scheduling") == "qos" ? 1 : 0)
        row("受信方式",scheduling)
        row("受信バッファ",buffer);row("映像同期の追加遅延 (0–250 ms)",delay);row("再生ゲイン (-96–0 dB)",gain)
        start.target = self;start.action = #selector(begin);stop.target = self;stop.action = #selector(end);stop.isEnabled = false
        let buttons = NSStackView(views:[start,stop,NSButton(title:"ログを開く",target:self,action:#selector(showLog))]);buttons.spacing = 12;stack.addArrangedSubview(buttons)
        metrics.font = .monospacedSystemFont(ofSize:12,weight:.regular);metrics.maximumNumberOfLines = 5;stack.addArrangedSubview(metrics)
        logLabel.font = .systemFont(ofSize:10);logLabel.textColor = .secondaryLabelColor;logLabel.maximumNumberOfLines = 2;stack.addArrangedSubview(logLabel)
        window.center();window.makeKeyAndOrderFront(nil);NSApp.activate(ignoringOtherApps:true)
    }
    func loadDevices() {
        let current = devices.titleOfSelectedItem ?? UserDefaults.standard.string(forKey:"device")
        let p = Process();p.executableURL = engine;p.arguments = ["--list-devices"]
        let pipe = Pipe();p.standardOutput = pipe
        do {try p.run();let data = pipe.fileHandleForReading.readDataToEndOfFile();p.waitUntilExit();devices.removeAllItems()
            for line in String(decoding:data,as:UTF8.self).split(separator:"\n") {
                if let d = try? JSONSerialization.jsonObject(with:Data(line.utf8)) as? [String:Any],let name = d["name"] as? String {devices.addItem(withTitle:name)}
            }
            if let current,devices.item(withTitle:current) != nil {devices.selectItem(withTitle:current)}
            else if let rme = devices.itemTitles.first(where:{$0.contains("Fireface UCX II")}){devices.selectItem(withTitle:rme)}
        } catch {status.stringValue = "デバイス取得失敗: \(error.localizedDescription)"}
    }
    @objc func refreshDevices(){loadDevices()}
    @objc func begin() {
        guard process == nil,let device = devices.titleOfSelectedItem else {return}
        guard let udpPort = UInt16(port.stringValue),udpPort>0,let ch = UInt32(channel.stringValue),ch>0,
              let av = Double(delay.stringValue),av.isFinite,(0...250).contains(av),
              let db = Double(gain.stringValue),db.isFinite,(-96...0).contains(db) else {status.stringValue = "ポート・チャンネル・遅延・ゲインの入力を確認してください";return}
        let prefs = UserDefaults.standard
        for (key,value) in [("device",device),("source",source.stringValue),("port",port.stringValue),("channel",channel.stringValue),("delay",delay.stringValue),("gain",gain.stringValue),("scheduling",scheduling.indexOfSelectedItem == 0 ? "realtime" : "qos"),("hardwareBuffer",hardwareBuffer.titleOfSelectedItem ?? "128 frames"),("buffer",buffer.titleOfSelectedItem ?? "20 ms")] {prefs.set(value,forKey:key)}
        let directory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/LAN-Audio-Receiver",isDirectory:true)
        do {
            try FileManager.default.createDirectory(at:directory,withIntermediateDirectories:true)
            let log = directory.appendingPathComponent("receiver-\(Int(Date().timeIntervalSince1970))-\(UUID().uuidString.prefix(8)).jsonl");logURL = log;logLabel.stringValue = log.path
            let p = Process();p.executableURL = engine
            p.arguments = ["--network-scheduling",scheduling.indexOfSelectedItem == 0 ? "realtime" : "qos","--device",device,"--bind","0.0.0.0:\(udpPort)","--channel",String(ch),"--buffer-frames",hardwareBuffer.indexOfSelectedItem == 0 ? "0" : (hardwareBuffer.titleOfSelectedItem ?? "128 frames").components(separatedBy:" ")[0],"--buffer-ms",(buffer.titleOfSelectedItem ?? "20 ms").components(separatedBy:" ")[0],"--av-sync-delay-ms",String(av),"--gain-db",String(db),"--output",log.path]
            if !source.stringValue.isEmpty {p.arguments! += ["--source",source.stringValue]}
            let stdout = Pipe();let stderr = Pipe();p.standardOutput = stdout;p.standardError = stderr;pending.removeAll()
            stdout.fileHandleForReading.readabilityHandler = { [weak self] handle in
                let data = handle.availableData
                if data.isEmpty {handle.readabilityHandler = nil;return}
                DispatchQueue.main.async {self?.consume(data)}
            }
            stderr.fileHandleForReading.readabilityHandler = { [weak self] handle in
                let data = handle.availableData
                if data.isEmpty {handle.readabilityHandler = nil;return}
                DispatchQueue.main.async {self?.details.stringValue = String(decoding:data,as:UTF8.self)}
            }
            p.terminationHandler = { [weak self] child in DispatchQueue.main.async {
                guard let self else {return}
                self.process = nil;self.setRunning(false);self.status.stringValue = child.terminationStatus == 0 ? "停止中" : "受信エラー (\(child.terminationStatus))"
                if self.terminating {NSApp.reply(toApplicationShouldTerminate:true)}
            }}
            try p.run();process = p;metrics.stringValue = "受信待機";setRunning(true);status.stringValue = "受信を開始しています…"
        } catch {status.stringValue = "起動失敗: \(error.localizedDescription)"}
    }
    func setRunning(_ running:Bool) {
        start.isEnabled = !running;stop.isEnabled = running
        for control in [devices,buffer,hardwareBuffer,scheduling,source,port,channel,delay,gain] as [NSControl] {control.isEnabled = !running}
    }
    func consume(_ data:Data) {
        pending.append(data)
        while let newline = pending.firstIndex(of:10) {
            let line = pending[..<newline];pending.removeSubrange(...newline)
            guard let d = try? JSONSerialization.jsonObject(with:Data(line)) as? [String:Any] else {continue}
            if d["event"] as? String == "receiver_start", let dev = d["device"] as? [String:Any] {
                details.stringValue = "\(dev["name"] ?? "") • \(dev["sample_rate"] ?? "") Hz • CoreAudio \(dev["buffer_frames"] ?? "") frames\n出力 \(channel.stringValue)/\((Int(channel.stringValue) ?? 1)+1) • ASRC有効 • ログにPCMは保存しません"
            }
            if d["event"] as? String == "receiver_scheduling" {
                let realtime = d["requested"] as? String == "Realtime" && (d["set_status"] as? Int) == 0 && (d["get_status"] as? Int) == 0 && (d["is_default"] as? Bool) == false
                details.stringValue += realtime ? " • 低遅延受信" : " • 通常QoS受信"
            }
            if let playout = d["playout"] as? [String:Any] {
                status.stringValue = d["status"] as? String == "receiving" ? "受信・再生中  \(d["peer"] ?? "")" : "Windowsからの音声を待っています"
                let fill = (playout["fill_ms"] as? NSNumber)?.doubleValue ?? 0
                let ppm = (playout["drift_ppm"] as? NSNumber)?.doubleValue ?? 0
                let peak = (d["network_peak"] as? NSNumber)?.doubleValue ?? 0
                metrics.stringValue = String(format:"受信 %@ packets  入力最大peak %.3f\nバッファ残量 %.2f ms  drift %+.1f ppm\n遅着 %@  欠落 %@ frames  underruns %@",String(describing:d["received_packets"] ?? 0),peak,fill,ppm,String(describing:playout["late_packets"] ?? 0),String(describing:playout["missing_frames"] ?? 0),String(describing:playout["underruns"] ?? 0))
                if let wait = d["kernel_queue_ms"] as? [String:Any] {
                    metrics.stringValue += String(format:"\n受信待ち p99 %.3f ms / 最大 %.3f ms",(wait["p99"] as? NSNumber)?.doubleValue ?? 0,(wait["max"] as? NSNumber)?.doubleValue ?? 0)
                }
            }
        }
    }
    @objc func end(){process?.interrupt();stop.isEnabled = false;status.stringValue = "停止しています…"}
    @objc func showLog(){if let logURL {NSWorkspace.shared.activateFileViewerSelecting([logURL])}}
    func applicationShouldTerminate(_ sender:NSApplication)->NSApplication.TerminateReply {
        if process != nil {terminating = true;end();return .terminateLater};return .terminateNow
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender:NSApplication)->Bool {true}
}
let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.run()
