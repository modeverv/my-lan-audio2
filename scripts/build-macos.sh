#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked -p receiver-macos
app="dist/LAN Audio Receiver.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/receiver-macos "$app/Contents/Resources/receiver-macos"
swiftc -O apps/receiver-gui/main.swift -o "$app/Contents/MacOS/LAN Audio Receiver" -framework AppKit
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.seijiro.LANAudioReceiver</string>
<key>CFBundleName</key><string>LAN Audio Receiver</string>
<key>CFBundleExecutable</key><string>LAN Audio Receiver</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSLocalNetworkUsageDescription</key><string>WindowsからLAN経由の音声を受信します。</string>
</dict></plist>
PLIST
codesign --force --sign - "$app/Contents/Resources/receiver-macos"
codesign --force --sign - "$app"
printf '%s\n' "Built: $app"
