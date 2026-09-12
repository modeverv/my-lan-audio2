# Application icon

`AppIcon.png` is the source image generated with the built-in image_gen tool on 2026-09-12.
It has a transparent background. `scripts/build-macos.sh` uses macOS sips/iconutil to produce
16–1024px representations in AppIcon.icns and embeds it in the app bundle.
CFBundleIconFile and NSApplication.applicationIconImage supply Finder and running Dock icons.

Generation prompt:

> Create one polished macOS desktop app icon for a low-latency LAN audio receiver. 1024x1024 square PNG with actual transparent background outside the icon. Centered large macOS rounded-square tile with standard modest transparent outer padding, deep midnight navy blue glossy ceramic base, sophisticated subtle bevel. A bold simple luminous cyan to violet audio waveform made of five rounded vertical bars in the center, connected horizontally to two small network endpoint dots on left and right. Strong silhouette readable at 32 pixels, high contrast, tasteful restrained depth, professional native Mac audio utility aesthetic, no text, no letters, no watermark, no extra objects, straight-on orthographic view, no scene/mockup.

The generated source is 1254×1254; build-time resizing preserves its alpha channel.

Windows shares this source: `scripts/build-windows-icon.ps1` generates a 16–256px
multi-resolution `dist/AppIcon.ico`. The sender build embeds it as both the EXE
icon and the WinForms window/taskbar icon.
