#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin ]] || { echo "App packaging requires macOS"; exit 1; }
BIN=target/x86_64-apple-darwin/release/noirraw
[[ -x "$BIN" ]] || { echo "Missing $BIN; cargo build --release --target x86_64-apple-darwin"; exit 1; }
mkdir -p dist/NoirRAW.app/Contents/MacOS
cp "$BIN" dist/NoirRAW.app/Contents/MacOS/noirraw
cat > dist/NoirRAW.app/Contents/Info.plist <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>noirraw</string>
<key>CFBundleIdentifier</key><string>app.noirraw.editor</string>
<key>CFBundleName</key><string>NoirRAW</string>
<key>CFBundleDisplayName</key><string>NoirRAW</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
/usr/bin/codesign --force --sign - --timestamp=none dist/NoirRAW.app
cd dist
/usr/bin/ditto -c -k --sequesterRsrc --keepParent NoirRAW.app NoirRAW-macOS-Intel.zip
echo "Wrote dist/NoirRAW-macOS-Intel.zip"
