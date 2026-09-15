import AppKit
import AVFoundation

func deviceName(_ device: SenderDevice) -> String {
    var value = device.name
    return withUnsafePointer(to: &value) { $0.withMemoryRebound(to: CChar.self, capacity: 256) { String(cString: $0) } }
}
func deviceUID(_ device: SenderDevice) -> String {
    var value = device.uid
    return withUnsafePointer(to: &value) { $0.withMemoryRebound(to: CChar.self, capacity: 256) { String(cString: $0) } }
}
func inputs() -> [SenderDevice] {
    var values = [SenderDevice](repeating: SenderDevice(), count: 128)
    let count = sender_devices(&values, 128)
    return Array(values.prefix(Int(count)))
}
let args = CommandLine.arguments
if args.contains("--list-devices") {
    for d in inputs() { print("\(d.id)\t\(deviceName(d))\t\(d.channels) ch\t\(Int(d.rate)) Hz\t\(d.buffer_frames) frames") }
    exit(0)
}
if args.count == 4 && args[1] == "--test-packets", let port = UInt16(args[3]) {
    exit(sender_test(args[2], port))
}
// Explicit diagnostic input capture, useful for independent UDP validation.
if args.count == 6 && args[1] == "--capture", let id = UInt32(args[2]), let port = UInt16(args[4]), let seconds = Double(args[5]), seconds > 0 {
    var error: Int32 = 0
    guard let sender = sender_start(id, 0, args[3], port, "", &error) else { fputs("Capture error: \(error)\n", stderr); exit(1) }
    Thread.sleep(forTimeInterval: seconds)
    var stats = SenderStats(); sender_stats(sender, &stats); sender_stop(sender)
    print("packets=\(stats.packets) frames=\(stats.frames) send_errors=\(stats.send_errors) capture_errors=\(stats.capture_errors)")
    exit(stats.packets > 0 && stats.send_errors == 0 && stats.capture_errors == 0 ? 0 : 1)
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    let device = NSPopUpButton()
    let channels = NSPopUpButton()
    let host = NSTextField(string: UserDefaults.standard.string(forKey: "host") ?? "239.255.0.1")
    let port = NSTextField(string: UserDefaults.standard.string(forKey: "port") ?? "40100")
    let interface = NSTextField(string: UserDefaults.standard.string(forKey: "interface") ?? "")
    let detail = NSTextField(labelWithString: "")
    let status = NSTextField(labelWithString: "停止中")
    let meter = NSLevelIndicator()
    let button = NSButton(title: "送信開始", target: nil, action: nil)
    let refresh = NSButton(title: "再読込", target: nil, action: nil)
    var devices: [SenderDevice] = []
    var sender: OpaquePointer?
    var timer: Timer?
    var lastFrames: UInt64 = 0
    var idleTicks = 0

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let iconURL = Bundle.main.url(forResource: "AppIcon", withExtension: "icns"),
           let icon = NSImage(contentsOf: iconURL) {
            NSApp.applicationIconImage = icon
        }
        let menu = NSMenu(); let appItem = NSMenuItem(); menu.addItem(appItem)
        let appMenu = NSMenu(); appMenu.addItem(withTitle: "LAN Audio Senderを終了", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        let editItem = NSMenuItem(); menu.addItem(editItem); let edit = NSMenu(title: "編集"); editItem.submenu = edit
        for (title, action, key) in [("コピー", "copy:", "c"), ("貼り付け", "paste:", "v"), ("すべて選択", "selectAll:", "a")] { edit.addItem(withTitle: title, action: Selector(action), keyEquivalent: key) }
        NSApp.mainMenu = menu
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 520, height: 380), styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "LAN Audio Sender"
        let stack = NSStackView(); stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 14
        stack.translatesAutoresizingMaskIntoConstraints = false; window.contentView!.addSubview(stack)
        NSLayoutConstraint.activate([stack.leadingAnchor.constraint(equalTo: window.contentView!.leadingAnchor, constant: 24), stack.trailingAnchor.constraint(equalTo: window.contentView!.trailingAnchor, constant: -24), stack.topAnchor.constraint(equalTo: window.contentView!.topAnchor, constant: 22)])
        let title = NSTextField(labelWithString: "Macの音声をLANへ送信"); title.font = .boldSystemFont(ofSize: 19); stack.addArrangedSubview(title)
        func row(_ label: String, _ views: [NSView]) {
            let name = NSTextField(labelWithString: label); name.widthAnchor.constraint(equalToConstant: 82).isActive = true
            let line = NSStackView(views: [name] + views); line.spacing = 8; stack.addArrangedSubview(line)
            line.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }
        device.target = self; device.action = #selector(deviceChanged)
        refresh.target = self; refresh.action = #selector(reload)
        row("音声入力", [device, refresh]); row("チャンネル", [channels]); row("送信先 IPv4", [host, port])
        port.widthAnchor.constraint(equalToConstant: 70).isActive = true
        interface.placeholderString = "自動（必要なら送信元IPv4）"; row("Multicast IF", [interface])
        detail.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular); stack.addArrangedSubview(detail)
        meter.minValue = 0; meter.maxValue = 1; meter.warningValue = 0.8; meter.criticalValue = 0.98
        meter.levelIndicatorStyle = .continuousCapacity; row("入力レベル", [meter])
        button.target = self; button.action = #selector(toggle); button.bezelStyle = .rounded
        row("", [button, status])
        let hint = NSTextField(wrappingLabelWithString: "Macの再生音を送る場合：再生アプリの出力と、この音声入力をBlackHoleに設定します。受信側の周波数を合わせてください。")
        hint.font = .systemFont(ofSize: 11); hint.textColor = .secondaryLabelColor; stack.addArrangedSubview(hint)
        hint.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        reload(); window.center(); window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in self?.update() }
    }
    @objc func reload() {
        let previous = device.indexOfSelectedItem
        let uid = previous >= 0 && previous < devices.count ? deviceUID(devices[previous]) : UserDefaults.standard.string(forKey: "deviceUID")
        devices = inputs(); device.removeAllItems(); device.addItems(withTitles: devices.map(deviceName))
        if let selected = devices.firstIndex(where: { deviceUID($0) == uid }) ?? devices.firstIndex(where: { deviceName($0).contains("BlackHole") }) { device.selectItem(at: selected) }
        deviceChanged()
    }
    @objc func deviceChanged() {
        channels.removeAllItems()
        guard device.indexOfSelectedItem >= 0 else { detail.stringValue = "音声入力が見つかりません"; button.isEnabled = false; return }
        button.isEnabled = true
        let d = devices[device.indexOfSelectedItem]
        if d.channels == 1 { channels.addItem(withTitle: "1 → L / R（モノラル）") }
        else { for i in 0..<Int(d.channels - 1) { channels.addItem(withTitle: "\(i + 1) / \(i + 2)") } }
        detail.stringValue = "\(Int(d.rate)) Hz · 入力 \(d.buffer_frames) frames · UDP 最大128 frames"
    }
    @objc func toggle() {
        if sender != nil { stop(); return }
        guard UInt16(port.stringValue) != nil, UInt16(port.stringValue) != 0 else { alert("ポートは1〜65535で指定してください。"); return }
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: start()
        case .notDetermined:
            button.isEnabled = false
            AVCaptureDevice.requestAccess(for: .audio) { granted in DispatchQueue.main.async { self.button.isEnabled = true; if granted { self.start() } else { self.alert("システム設定 → プライバシーとセキュリティ → マイクで許可してください。") } } }
        default: alert("システム設定 → プライバシーとセキュリティ → マイクでLAN Audio Senderを許可してください。")
        }
    }
    func start() {
        guard device.indexOfSelectedItem >= 0, let portNumber = UInt16(port.stringValue) else { return }
        let d = devices[device.indexOfSelectedItem]; var error: Int32 = 0
        sender = sender_start(d.id, UInt32(max(0, channels.indexOfSelectedItem)), host.stringValue.trimmingCharacters(in: .whitespaces), portNumber, interface.stringValue.trimmingCharacters(in: .whitespaces), &error)
        guard sender != nil else { alert("送信を開始できません（エラー \(error)）。入力デバイスとIPv4アドレスを確認してください。"); return }
        saveSettings()
        for control in [device, channels, host, port, interface, refresh] as [NSControl] { control.isEnabled = false }
        lastFrames = 0; idleTicks = 0; button.title = "送信停止"; status.stringValue = "入力待ち"
    }
    func saveSettings() {
        if device.indexOfSelectedItem >= 0 { UserDefaults.standard.set(deviceUID(devices[device.indexOfSelectedItem]), forKey: "deviceUID") }
        for (key, value) in [("host", host.stringValue), ("port", port.stringValue), ("interface", interface.stringValue)] { UserDefaults.standard.set(value, forKey: key) }
    }
    func stop() {
        if let value = sender { sender_stop(value) }; sender = nil
        for control in [device, channels, host, port, interface, refresh] as [NSControl] { control.isEnabled = true }
        button.title = "送信開始"; status.stringValue = "停止中"; meter.doubleValue = 0
    }
    func update() {
        guard let sender else { return }
        var stats = SenderStats(); sender_stats(sender, &stats); meter.doubleValue = Double(stats.peak)
        idleTicks = stats.frames == lastFrames ? idleTicks + 1 : 0; lastFrames = stats.frames
        if stats.send_errors + stats.capture_errors > 0 { status.stringValue = "エラー \(stats.last_error)（送信 \(stats.send_errors) / 入力 \(stats.capture_errors)）" }
        else if idleTicks >= 8 { status.stringValue = "入力なし：デバイス・権限を確認" }
        else { status.stringValue = "送信中 · \(stats.packets) packets" }
    }
    func alert(_ text: String) { let alert = NSAlert(); alert.messageText = text; alert.runModal() }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
    func applicationWillTerminate(_ notification: Notification) { timer?.invalidate(); saveSettings(); stop() }
}
if args.count > 1 {
    fputs("Usage: LAN Audio Sender [--list-devices | --test-packets IPv4 PORT | --capture DEVICE IPv4 PORT SECONDS]\n", stderr)
    exit(2)
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = AppDelegate(); app.delegate = delegate; app.run()
