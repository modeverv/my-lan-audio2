# Sender application icon

`AppIcon.png` was generated with the built-in image_gen tool on 2026-09-15.
The receiver icon was used as a style reference. The outgoing arrow distinguishes
this sender from the receiver. The transparent source is kept here, and
`scripts/build-macos-sender.sh` converts it to 16–1024px ICNS representations.
`CFBundleIconFile` supplies the Finder icon, and `NSApp.applicationIconImage`
supplies the running Dock icon.

Generation prompt:

> Use case: logo-brand. Create one polished macOS desktop app icon for LAN Audio Sender, a companion to the reference LAN audio receiver icon. Reference image role: style reference only. Keep the deep midnight-blue glossy rounded-square tile, modest transparent outer padding and luminous cyan/violet audio utility aesthetic. Make a distinct simple bold symbol: three rounded vertical audio waveform bars flowing into a clear right-pointing arrow, indicating outgoing audio to the network. The waveform and arrow should be a single balanced centered composition, readable at 32px. Restrained glossy depth, high contrast, straight-on orthographic view. 1024x1024 square PNG, genuinely transparent outside the rounded-square tile. No text, no letters, no watermark, no extra objects, no scene or mockup. Save the generated result for integration as the source app icon.
