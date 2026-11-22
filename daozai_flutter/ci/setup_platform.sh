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

    # 动态读取 Flutter 实际生成的 applicationId（org + project_name），
    # 避免小组件 Provider 包名 / R 引用与工程不一致导致 Kotlin 编译失败。
    GRADLE="android/app/build.gradle"
    GRADLE_KTS="android/app/build.gradle.kts"
    APP_PKG=""
    if [ -f "$GRADLE_KTS" ]; then
      APP_PKG=$(grep -oE 'applicationId\s*=\s*"[^"]+"' "$GRADLE_KTS" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
    fi
    if [ -z "$APP_PKG" ] && [ -f "$GRADLE" ]; then
      APP_PKG=$(grep -oE 'applicationId\s+"[^"]+"' "$GRADLE" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
    fi
    # 兜底：从 MainActivity.kt 的 package 声明读取。
    if [ -z "$APP_PKG" ]; then
      MAIN=$(find android/app/src/main/kotlin -name 'MainActivity.kt' 2>/dev/null | head -1)
      [ -n "$MAIN" ] && APP_PKG=$(grep -oE '^package [a-zA-Z0-9_.]+' "$MAIN" | head -1 | awk '{print $2}')
    fi
    [ -z "$APP_PKG" ] && APP_PKG="com.daozai.daozai_flutter"
    echo "[android] applicationId = $APP_PKG"

    PKG_PATH=$(echo "$APP_PKG" | tr '.' '/')
    KOTLIN="android/app/src/main/kotlin/$PKG_PATH/widget"
    mkdir -p "$RES/layout" "$RES/drawable" "$RES/xml" "$RES/values" "$KOTLIN"

    cp platform/android_widget/res/layout/daozai_widget.xml "$RES/layout/"
    cp platform/android_widget/res/drawable/widget_bg.xml "$RES/drawable/"
    cp platform/android_widget/res/xml/daozai_widget_info.xml "$RES/xml/"
    # 替换包名占位符后放入正确包路径。
    sed "s#__APP_PACKAGE__#$APP_PKG#g" platform/android_widget/kotlin/DaozaiWidgetProvider.kt > "$KOTLIN/DaozaiWidgetProvider.kt"

    # 合并小组件描述字符串（独立文件，避免改动 Flutter 生成的 strings.xml）。
    cp platform/android_widget/res/values/strings.xml "$RES/values/daozai_widget_strings.xml"

    # 合并 <receiver> 到 AndroidManifest.xml 的 </application> 前。
    MANIFEST="android/app/src/main/AndroidManifest.xml"
    if [ -f "$MANIFEST" ] && ! grep -q "DaozaiWidgetProvider" "$MANIFEST"; then
      RECEIVER='        <receiver android:name=".widget.DaozaiWidgetProvider" android:exported="true" android:label="Daozai">\n            <intent-filter>\n                <action android:name="android.appwidget.action.APPWIDGET_UPDATE" />\n            </intent-filter>\n            <meta-data android:name="android.appwidget.provider" android:resource="@xml/daozai_widget_info" />\n        </receiver>\n'
      sedi "s#</application>#${RECEIVER}    </application>#" "$MANIFEST"
    fi

    # 待办本地提醒所需权限（Android 13+ 通知权限、精确闹钟、唤醒锁）。
    if [ -f "$MANIFEST" ] && ! grep -q "POST_NOTIFICATIONS" "$MANIFEST"; then
      PERMS='    <uses-permission android:name="android.permission.POST_NOTIFICATIONS"/>\n    <uses-permission android:name="android.permission.SCHEDULE_EXACT_ALARM"/>\n    <uses-permission android:name="android.permission.USE_EXACT_ALARM"/>\n    <uses-permission android:name="android.permission.WAKE_LOCK"/>\n    <uses-permission android:name="android.permission.RECEIVE_BOOT_COMPLETED"/>\n'
      sedi "s#<application#${PERMS}    <application#" "$MANIFEST"
    fi
    # flutter_local_notifications 的定时提醒 receiver（应用重启后恢复提醒）。
    if [ -f "$MANIFEST" ] && ! grep -q "ScheduledNotificationReceiver" "$MANIFEST"; then
      NOTIF_RECEIVER='        <receiver android:exported="false" android:name="com.dexterous.flutterlocalnotifications.ScheduledNotificationReceiver"/>\n        <receiver android:exported="false" android:name="com.dexterous.flutterlocalnotifications.ScheduledNotificationBootReceiver">\n            <intent-filter>\n                <action android:name="android.intent.action.BOOT_COMPLETED"/>\n                <action android:name="android.intent.action.MY_PACKAGE_REPLACED"/>\n                <action android:name="android.intent.action.QUICKBOOT_POWERON"/>\n            </intent-filter>\n        </receiver>\n'
      sedi "s#</application>#${NOTIF_RECEIVER}    </application>#" "$MANIFEST"
    fi
    echo "[android] 小组件 + 通知权限注入完成。"
    ;;

  *)
    echo "未知平台: $PLATFORM（应为 windows|macos|android）" >&2
    exit 1
    ;;
esac