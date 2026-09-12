import AppKit
import Darwin

private let receiverAccent = NSColor(srgbRed: 0.05, green: 0.80, blue: 0.73, alpha: 1)
private let receiverBackground = NSColor(srgbRed: 0.045, green: 0.065, blue: 0.075, alpha: 1)

final class ReceiverButton: NSButton {
    var primary = false
    override var isEnabled: Bool { didSet { needsDisplay = true } }
    override func draw(_ dirtyRect: NSRect) {
        let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: 12, yRadius: 12)
        let fill = primary && isEnabled ? receiverAccent : NSColor(white: 0.13, alpha: 1)
        (isHighlighted ? fill.blended(withFraction: 0.18, of: .white)! : fill).setFill()
        shape.fill()
        let foreground: NSColor = !isEnabled ? NSColor(white: 0.38, alpha: 1) : primary ? receiverBackground : NSColor(white: 0.88, alpha: 1)
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 14, weight: .semibold), .foregroundColor: foreground]
        let size = (title as NSString).size(withAttributes: attributes)
        (title as NSString).draw(at: NSPoint(x: (bounds.width-size.width)/2, y: (bounds.height-size.height)/2), withAttributes: attributes)
        if window?.firstResponder === self {
            receiverAccent.setStroke(); shape.lineWidth = 2; shape.stroke()
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    var window: NSWindow!
    var settingsWindow: NSWindow!
    let refresh = NSButton(title: "デバイス一覧を更新", target: nil, action: nil)
    var process: Process?
    var terminating = false
    var pending = Data()
    let devices = NSPopUpButton()
    let buffer = NSPopUpButton()
    let hardwareBuffer = NSPopUpButton()
    let scheduling = NSPopUpButton()
    let source = NSTextField(string: UserDefaults.standard.string(forKey: "source") ?? "")
    let port = NSTextField(string: UserDefaults.standard.string(forKey: "port") ?? "40100")
    let multicastGroup = NSTextField(string: UserDefaults.standard.string(forKey: "multicastGroup") ?? "")
    let multicastInterface = NSTextField(string: UserDefaults.standard.string(forKey: "multicastInterface") ?? "")
    let channel = NSTextField(string: UserDefaults.standard.string(forKey: "channel") ?? "1")
    let delay = NSTextField(string: UserDefaults.standard.string(forKey: "delay") ?? "0")
    let gain = NSTextField(string: UserDefaults.standard.string(forKey: "gain") ?? "-12")
    let start = ReceiverButton(title: "受信開始", target: nil, action: nil)
    let stop = ReceiverButton(title: "停止", target: nil, action: nil)
    let mainStatus = NSTextField(labelWithString: "STOPPED")
    let mainCaption = NSTextField(labelWithString: "受信停止中")
    let status = NSTextField(labelWithString: "停止中")
    let details = NSTextField(wrappingLabelWithString: "Windowsの送信先はMacのLAN IP、または下記のマルチキャストIPです。\n受信開始で、選択したCoreAudioデバイスへ再生します。")
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
        let submenu = NSMenu()
        let settingsItem = submenu.addItem(withTitle: "設定…", action: #selector(showSettings), keyEquivalent: ",")
        settingsItem.target = self
        submenu.addItem(.separator())
        submenu.addItem(withTitle: "LAN Audio Receiverを終了", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q");appMenu.submenu = submenu
        NSApp.mainMenu = menu
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 340, height: 218), styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "LAN Audio Receiver"
        window.isReleasedWhenClosed = false
        window.delegate = self
        start.target = self; start.action = #selector(begin)
        stop.target = self; stop.action = #selector(end); stop.isEnabled = false
        let settingsButton = NSButton(image: NSImage(systemSymbolName: "gearshape", accessibilityDescription: "設定")!, target: self, action: #selector(showSettings))
        settingsButton.isBordered = false
        settingsButton.contentTintColor = .secondaryLabelColor
        settingsButton.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 20, weight: .regular)
        settingsButton.toolTip = "設定（⌘,）"
        settingsButton.setAccessibilityLabel("設定")
        window.appearance = NSAppearance(named: .darkAqua)
        window.backgroundColor = receiverBackground
        window.titlebarAppearsTransparent = true
        let content = window.contentView!
        let heading = NSTextField(labelWithString: "LAN AUDIO")
        heading.font = .systemFont(ofSize: 13, weight: .bold)
        heading.textColor = NSColor(white: 0.60, alpha: 1)
        mainStatus.font = .systemFont(ofSize: 35, weight: .bold)
        mainStatus.alignment = .center
        mainStatus.textColor = NSColor(white: 0.60, alpha: 1)
        mainCaption.font = .systemFont(ofSize: 12, weight: .medium)
        mainCaption.alignment = .center
        mainCaption.textColor = .secondaryLabelColor
        start.primary = true
        for button in [start, stop] { button.isBordered = false; button.setButtonType(.momentaryPushIn) }
        let buttons = NSStackView(views: [start, stop])
        buttons.spacing = 12
        buttons.distribution = .fillEqually
        for view in [heading, settingsButton, mainStatus, mainCaption, buttons] {
            view.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(view)
        }
        NSLayoutConstraint.activate([
            heading.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 24),
            heading.topAnchor.constraint(equalTo: content.topAnchor, constant: 17),
            settingsButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            settingsButton.centerYAnchor.constraint(equalTo: heading.centerYAnchor),
            settingsButton.widthAnchor.constraint(equalToConstant: 28),
            settingsButton.heightAnchor.constraint(equalToConstant: 28),
            mainStatus.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 20),
            mainStatus.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            mainStatus.topAnchor.constraint(equalTo: content.topAnchor, constant: 61),
            mainCaption.centerXAnchor.constraint(equalTo: content.centerXAnchor),
            mainCaption.topAnchor.constraint(equalTo: mainStatus.bottomAnchor, constant: 5),
            buttons.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 24),
            buttons.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24),
            buttons.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -24),
            start.heightAnchor.constraint(equalToConstant: 42),
            stop.heightAnchor.constraint(equalToConstant: 42)
        ])
        settingsWindow = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 700, height: 750), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        settingsWindow.appearance = NSAppearance(named: .darkAqua)
        settingsWindow.backgroundColor = receiverBackground
        settingsWindow.titlebarAppearsTransparent = true
        settingsWindow.title = "LAN Audio Receiver — 設定"
        settingsWindow.isReleasedWhenClosed = false
        settingsWindow.delegate = self
        let stack = NSStackView();stack.orientation = .vertical;stack.alignment = .leading;stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        settingsWindow.contentView!.addSubview(stack)
        NSLayoutConstraint.activate([stack.leadingAnchor.constraint(equalTo:settingsWindow.contentView!.leadingAnchor,constant:24),stack.trailingAnchor.constraint(equalTo:settingsWindow.contentView!.trailingAnchor,constant:-24),stack.topAnchor.constraint(equalTo:settingsWindow.contentView!.topAnchor,constant:22)])
        let title = NSTextField(labelWithString: "設定");title.font = .systemFont(ofSize:25,weight:.semibold);stack.addArrangedSubview(title)
        status.font = .systemFont(ofSize:15,weight:.medium);stack.addArrangedSubview(status)
        details.font = .systemFont(ofSize:12);details.textColor = .secondaryLabelColor;stack.addArrangedSubview(details)
        func row(_ label:String,_ view:NSView) {let text = NSTextField(labelWithString:label);text.widthAnchor.constraint(equalToConstant:240).isActive = true;view.widthAnchor.constraint(equalToConstant:395).isActive = true;let r = NSStackView(views:[text,view]);r.spacing = 12;stack.addArrangedSubview(r)}
        loadDevices()
        row("出力デバイス",devices)
        refresh.target = self;refresh.action = #selector(refreshDevices);stack.addArrangedSubview(refresh)
        row("Windows IP（空欄で自動）",source);row("UDPポート",port);row("出力チャンネル先頭（1始まり）",channel)
        multicastGroup.placeholderString = "空欄: ユニキャスト / 例: 239.255.0.1"
        multicastInterface.placeholderString = "空欄: 自動 / 例: 192.168.11.65"
        row("マルチキャストIP",multicastGroup)
        row("受信LAN（MacのIPv4）",multicastInterface)
        buffer.addItems(withTitles:["2 ms","5 ms","10 ms","20 ms","40 ms"]);buffer.selectItem(withTitle:UserDefaults.standard.string(forKey:"buffer") ?? "20 ms")
        hardwareBuffer.addItems(withTitles:["現在の設定を維持", "16 frames", "32 frames", "64 frames", "128 frames", "256 frames", "512 frames"]);hardwareBuffer.selectItem(withTitle:UserDefaults.standard.string(forKey:"hardwareBuffer") ?? "128 frames")
        row("CoreAudioバッファ（停止時に復元）",hardwareBuffer)
        scheduling.addItems(withTitles:["低遅延（リアルタイム）", "標準（比較用）"])
        scheduling.selectItem(at:UserDefaults.standard.string(forKey:"scheduling") == "qos" ? 1 : 0)
        row("受信方式",scheduling)
        row("受信バッファ",buffer);row("映像同期の追加遅延 (0–250 ms)",delay);row("再生ゲイン (-96–0 dB)",gain)
        let hint = NSTextField(labelWithString: "変更は次の受信開始時に反映されます。受信中は設定を変更できません。")
        hint.font = .systemFont(ofSize: 12); hint.textColor = .secondaryLabelColor
        stack.addArrangedSubview(hint)
        stack.addArrangedSubview(NSButton(title: "ログを開く", target: self, action: #selector(showLog)))
        metrics.font = .monospacedSystemFont(ofSize:12,weight:.regular);metrics.maximumNumberOfLines = 5;stack.addArrangedSubview(metrics)
        logLabel.font = .systemFont(ofSize:10);logLabel.textColor = .secondaryLabelColor;logLabel.maximumNumberOfLines = 2;stack.addArrangedSubview(logLabel)
        for label in [status, details, metrics, logLabel] {
            label.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
            label.lineBreakMode = .byWordWrapping
            label.maximumNumberOfLines = 0
        }
        settingsWindow.center()
        window.center();window.makeKeyAndOrderFront(nil);NSApp.activate(ignoringOtherApps:true)
    }
    @objc func showSettings() {
        settingsWindow.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
    func windowWillClose(_ notification: Notification) {
        if let closed = notification.object as? NSWindow, closed === settingsWindow {
            settingsWindow.makeFirstResponder(nil)
            saveSettings()
        }
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if sender === window { NSApp.terminate(nil); return false }
        return true
    }
    func showError(_ message: String) {
        status.stringValue = message
        updateMainStatus("ERROR", caption: "設定を確認してください", active: false)
        showSettings()
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
    @objc func refreshDevices(){if process == nil {loadDevices()}}
    @objc func begin() {
        guard process == nil else {return}
        settingsWindow.makeFirstResponder(nil)
        guard let device = devices.titleOfSelectedItem else {showError("出力デバイスを選択してください");return}
        guard let udpPort = UInt16(port.stringValue),udpPort>0,let ch = UInt32(channel.stringValue),ch>0,
              let av = Double(delay.stringValue),av.isFinite,(0...250).contains(av),
              let db = Double(gain.stringValue),db.isFinite,(-96...0).contains(db) else {showError("ポート・チャンネル・遅延・ゲインの入力を確認してください");return}
        multicastGroup.stringValue = multicastGroup.stringValue.trimmingCharacters(in:.whitespacesAndNewlines)
        multicastInterface.stringValue = multicastInterface.stringValue.trimmingCharacters(in:.whitespacesAndNewlines)
        func ipv4(_ text:String)->[UInt8]? {
            let parts = text.split(separator:".",omittingEmptySubsequences:false)
            guard parts.count == 4 else {return nil}
            let bytes = parts.compactMap {UInt8($0)}
            return bytes.count == 4 ? bytes : nil
        }
        if !multicastGroup.stringValue.isEmpty {
            guard let group = ipv4(multicastGroup.stringValue),(224...239).contains(Int(group[0])) else {
                showError("マルチキャストIPは224.0.0.0〜239.255.255.255を指定してください");return
            }
            if !multicastInterface.stringValue.isEmpty {
                guard let address = ipv4(multicastInterface.stringValue),address[0]<224 else {
                    showError("受信LANにはMacのIPv4アドレスを指定してください");return
                }
            }
        } else if !multicastInterface.stringValue.isEmpty {
            showError("受信LANを指定する場合はマルチキャストIPも入力してください");return
        }
        saveSettings()
        let directory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/LAN-Audio-Receiver",isDirectory:true)
        do {
            try FileManager.default.createDirectory(at:directory,withIntermediateDirectories:true)
            let log = directory.appendingPathComponent("receiver-\(Int(Date().timeIntervalSince1970))-\(UUID().uuidString.prefix(8)).jsonl");logURL = log;logLabel.stringValue = log.path
            let p = Process();p.executableURL = engine
            p.arguments = ["--network-scheduling",scheduling.indexOfSelectedItem == 0 ? "realtime" : "qos","--device",device,"--bind","0.0.0.0:\(udpPort)","--channel",String(ch),"--buffer-frames",hardwareBuffer.indexOfSelectedItem == 0 ? "0" : (hardwareBuffer.titleOfSelectedItem ?? "128 frames").components(separatedBy:" ")[0],"--buffer-ms",(buffer.titleOfSelectedItem ?? "20 ms").components(separatedBy:" ")[0],"--av-sync-delay-ms",String(av),"--gain-db",String(db),"--output",log.path]
            if !source.stringValue.isEmpty {p.arguments! += ["--source",source.stringValue]}
            if !multicastGroup.stringValue.isEmpty {
                p.arguments! += ["--multicast-group",multicastGroup.stringValue]
                if !multicastInterface.stringValue.isEmpty {p.arguments! += ["--multicast-interface",multicastInterface.stringValue]}
            }
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
                if child.terminationStatus != 0 && !self.terminating {self.showSettings()}
                if self.terminating {NSApp.reply(toApplicationShouldTerminate:true)}
            }}
            if !multicastGroup.stringValue.isEmpty {
                // Receive-only BSD sockets may stay silent without prompting for local
                // network access. A foreground UDP connect to the group discard port requests
                // access for this app; no audio or user data is sent by the probe.
                let fd = socket(AF_INET, SOCK_DGRAM, 0)
                if fd >= 0 {
                    var destination = sockaddr_in()
                    destination.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
                    destination.sin_family = sa_family_t(AF_INET)
                    destination.sin_port = UInt16(9).bigEndian
                    inet_pton(AF_INET, multicastGroup.stringValue, &destination.sin_addr)
                    if !multicastInterface.stringValue.isEmpty {
                        var interface = in_addr()
                        inet_pton(AF_INET, multicastInterface.stringValue, &interface)
                        setsockopt(fd, IPPROTO_IP, IP_MULTICAST_IF, &interface, socklen_t(MemoryLayout<in_addr>.size))
                    }
                    withUnsafePointer(to:&destination) { pointer in
                        pointer.withMemoryRebound(to:sockaddr.self,capacity:1) {
                            _ = connect(fd,$0,socklen_t(MemoryLayout<sockaddr_in>.size))
                        }
                    }
                    close(fd)
                }
            }
            try p.run();process = p;metrics.stringValue = "受信待機";setRunning(true);status.stringValue = "受信を開始しています…"
        } catch {showError("起動失敗: \(error.localizedDescription)")}
    }
    func saveSettings() {
        guard let device = devices.titleOfSelectedItem else { return }
        let prefs = UserDefaults.standard
        for (key,value) in [("multicastGroup",multicastGroup.stringValue),("multicastInterface",multicastInterface.stringValue),("device",device),("source",source.stringValue),("port",port.stringValue),("channel",channel.stringValue),("delay",delay.stringValue),("gain",gain.stringValue),("scheduling",scheduling.indexOfSelectedItem == 0 ? "realtime" : "qos"),("hardwareBuffer",hardwareBuffer.titleOfSelectedItem ?? "128 frames"),("buffer",buffer.titleOfSelectedItem ?? "20 ms")] {prefs.set(value,forKey:key)}
    }
    func updateMainStatus(_ title: String, caption: String, active: Bool) {
        mainStatus.stringValue = title
        mainStatus.textColor = active ? receiverAccent : NSColor(white: 0.60, alpha: 1)
        mainCaption.stringValue = caption
    }
    func setRunning(_ running:Bool) {
        updateMainStatus(running ? "WAITING" : "STOPPED", caption: running ? "音声を待っています" : "受信停止中", active: false)
        start.isEnabled = !running;stop.isEnabled = running
        refresh.isEnabled = !running
        window.title = running ? "LAN Audio Receiver — 受信中" : "LAN Audio Receiver"
        for control in [devices,buffer,hardwareBuffer,scheduling,source,port,multicastGroup,multicastInterface,channel,delay,gain] as [NSControl] {control.isEnabled = !running}
    }
    func consume(_ data:Data) {
        pending.append(data)
        while let newline = pending.firstIndex(of:10) {
            let line = pending[..<newline];pending.removeSubrange(...newline)
            guard let d = try? JSONSerialization.jsonObject(with:Data(line)) as? [String:Any] else {continue}
            if d["event"] as? String == "receiver_start", let dev = d["device"] as? [String:Any] {
                details.stringValue = "\(dev["name"] ?? "") • \(dev["sample_rate"] ?? "") Hz • CoreAudio \(dev["buffer_frames"] ?? "") frames\n出力 \(channel.stringValue)/\((Int(channel.stringValue) ?? 1)+1) • ASRC有効 • ログにPCMは保存しません"
            }
            if d["event"] as? String == "receiver_start",let group = d["multicast_group"] as? String {
                details.stringValue += " • Multicast \(group)"
            }
            if d["event"] as? String == "receiver_scheduling" {
                let realtime = d["requested"] as? String == "Realtime" && (d["set_status"] as? Int) == 0 && (d["get_status"] as? Int) == 0 && (d["is_default"] as? Bool) == false
                details.stringValue += realtime ? " • 低遅延受信" : " • 通常QoS受信"
            }
            if let playout = d["playout"] as? [String:Any] {
                if stop.isEnabled {
                    let receiving = d["status"] as? String == "receiving"
                    updateMainStatus(receiving ? "RECEIVING" : "WAITING", caption: receiving ? "受信・再生中" : "音声を待っています", active: receiving)
                }
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
    @objc func end(){updateMainStatus("STOPPING", caption: "停止しています…", active: false);process?.interrupt();stop.isEnabled = false;status.stringValue = "停止しています…"}
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
