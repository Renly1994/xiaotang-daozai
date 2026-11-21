#!/usr/bin/env bash
# 平台配置注入 —— 由 CI 在 `flutter create` 生成平台工程后调用。
# 用法: bash ci/setup_platform.sh <windows|macos|android>
set -euo pipefail

PLATFORM="${1:-}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# 跨平台原地 sed：macOS 用 -i ''，GNU/Linux 用 -i。
sedi() {
  if [[ "$(uname)" == "Darwin" ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

case "$PLATFORM" in
  windows)
    echo "[windows] 透明窗口由 window_manager 运行时设置，无需额外注入。"
    ;;

  macos)
    echo "[macos] 注入联网权限（天气请求）..."
    for ENT in macos/Runner/DebugProfile.entitlements macos/Runner/Release.entitlements; do
      if [ -f "$ENT" ]; then
        # 在 </dict> 前插入 network.client（若尚未存在）。
        if ! grep -q "com.apple.security.network.client" "$ENT"; then
          sedi 's#</dict>#\t<key>com.apple.security.network.client</key>\n\t<true/>\n</dict>#' "$ENT"
        fi
      fi
    done
    echo "[macos] 完成。透明无边框由 window_manager setAsFrameless 运行时处理。"
    ;;

  android)
    echo "[android] 注入主屏小组件..."
    RES="android/app/src/main/res"
    KOTLIN="android/app/src/main/kotlin/com/daozai/flutter/widget"
    mkdir -p "$RES/layout" "$RES/drawable" "$RES/xml" "$RES/values" "$KOTLIN"

    cp platform/android_widget/res/layout/daozai_widget.xml "$RES/layout/"
    cp platform/android_widget/res/drawable/widget_bg.xml "$RES/drawable/"
    cp platform/android_widget/res/xml/daozai_widget_info.xml "$RES/xml/"
    cp platform/android_widget/kotlin/DaozaiWidgetProvider.kt "$KOTLIN/"

    # 合并小组件描述字符串。
    STRINGS="$RES/values/strings.xml"
    if [ -f "$STRINGS" ]; then
      if ! grep -q "widget_description" "$STRINGS"; then
        sedi 's#</resources>#    <string name="widget_description">岛仔：时间、天气与待办，抬眼即见。</string>\n</resources>#' "$STRINGS"
      fi
    else
      cp platform/android_widget/res/values/strings.xml "$STRINGS"
    fi

    # 合并 <receiver> 到 AndroidManifest.xml 的 </application> 前。
    MANIFEST="android/app/src/main/AndroidManifest.xml"
    if [ -f "$MANIFEST" ] && ! grep -q "DaozaiWidgetProvider" "$MANIFEST"; then
      RECEIVER='        <receiver android:name=".widget.DaozaiWidgetProvider" android:exported="true" android:label="岛仔">\n            <intent-filter>\n                <action android:name="android.appwidget.action.APPWIDGET_UPDATE" />\n            </intent-filter>\n            <meta-data android:name="android.appwidget.provider" android:resource="@xml/daozai_widget_info" />\n        </receiver>\n'
      sedi "s#</application>#${RECEIVER}    </application>#" "$MANIFEST"
    fi
    echo "[android] 小组件注入完成。"
    ;;

  *)
    echo "未知平台: $PLATFORM（应为 windows|macos|android）" >&2
    exit 1
    ;;
esac