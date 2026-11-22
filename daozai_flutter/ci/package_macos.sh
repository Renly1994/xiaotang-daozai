#!/usr/bin/env bash
# 打包可直接挂载的 DMG，并验证其中确实只有 DynamicIsland.app。
set -euo pipefail

APP="build/macos/Build/Products/Release/DynamicIsland.app"
DMG="DynamicIsland-macos.dmg"
STAGE="$(mktemp -d)"
MOUNT="$(mktemp -d)"
ATTACHED=0

cleanup() {
  if [ "$ATTACHED" = "1" ]; then
    hdiutil detach "$MOUNT" -quiet || true
  fi
  rm -rf "$STAGE" "$MOUNT"
}
trap cleanup EXIT

test -d "$APP"
rm -f "$DMG"
mkdir -p "$STAGE/DynamicIsland"
ditto "$APP" "$STAGE/DynamicIsland/DynamicIsland.app"

hdiutil create -volname "DynamicIsland" -srcfolder "$STAGE/DynamicIsland" \
  -format UDZO -ov "$DMG"
hdiutil verify "$DMG"
hdiutil attach -readonly -nobrowse "$DMG" -mountpoint "$MOUNT" -quiet
ATTACHED=1
test -d "$MOUNT/DynamicIsland.app"
plutil -extract CFBundlePackageType raw "$MOUNT/DynamicIsland.app/Contents/Info.plist" | grep -qx APPL

shasum -a 256 "$DMG" > SHA256SUMS.txt
cat > README-macOS.txt <<'EOF'
DynamicIsland macOS 安装

1. 解压 GitHub 下载的外层 ZIP。
2. 双击其中的 DynamicIsland-macos.dmg。
3. 把 DynamicIsland.app 拖入“应用程序”文件夹。
4. 首次打开：按住 Control 点击 DynamicIsland.app，再点“打开”。

外层 ZIP 是 GitHub Actions 的制品包装。
DMG 才是实际安装包，不要再解压 DMG。
EOF
