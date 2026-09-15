#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
app="dist/LAN Audio Sender.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" target/macos-sender
iconset="target/macos-sender/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" apps/sender-macos/Resources/AppIcon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    retina=$((size * 2))
    sips -z "$retina" "$retina" apps/sender-macos/Resources/AppIcon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
MACOSX_DEPLOYMENT_TARGET=13.0 clang -std=c11 -O2 -Wall -Wextra -Werror -c apps/sender-macos/sender.c -o target/macos-sender/sender.o
MACOSX_DEPLOYMENT_TARGET=13.0 swiftc -O apps/sender-macos/main.swift target/macos-sender/sender.o -import-objc-header apps/sender-macos/sender.h -o "$app/Contents/MacOS/LAN Audio Sender" -framework AppKit -framework AVFoundation -framework AudioToolbox -framework CoreAudio
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.seijiro.LANAudioSender</string>
<key>CFBundleName</key><string>LAN Audio Sender</string>
<key>CFBundleExecutable</key><string>LAN Audio Sender</string>
<key>CFBundleIconFile</key><string>AppIcon.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>2</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSMicrophoneUsageDescription</key><string>選択した音声入力（BlackHole・オーディオインターフェース・マイク）をLANへ送信します。</string>
<key>NSLocalNetworkUsageDescription</key><string>LAN内の受信機へ音声を送信します。</string>
</dict></plist>
PLIST
codesign --force --sign "${SIGNING_IDENTITY:--}" "$app"
printf '%s\n' "Built: $app"
