#!/bin/sh
# Build release and assemble Liber.app. Usage: packaging/bundle.sh [dest-dir]
set -e
cd "$(dirname "$0")/.."
DEST="${1:-target/bundle}"
cargo build --release
APP="$DEST/Liber.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/liber-gpui "$APP/Contents/MacOS/Liber"
cp packaging/Liber.icns "$APP/Contents/Resources/Liber.icns"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Liber</string>
<key>CFBundleDisplayName</key><string>Liber</string>
<key>CFBundleIdentifier</key><string>app.liber.reader</string>
<key>CFBundleExecutable</key><string>Liber</string>
<key>CFBundleIconFile</key><string>Liber</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$VERSION</string>
<key>CFBundleVersion</key><string>$VERSION</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>CFBundleDocumentTypes</key><array><dict>
<key>CFBundleTypeName</key><string>PDF</string>
<key>CFBundleTypeRole</key><string>Viewer</string>
<key>LSHandlerRank</key><string>Alternate</string>
<key>LSItemContentTypes</key><array><string>com.adobe.pdf</string></array>
</dict></array>
</dict></plist>
PLIST
codesign --force --deep --sign - "$APP" 2>/dev/null || true
echo "$APP"
