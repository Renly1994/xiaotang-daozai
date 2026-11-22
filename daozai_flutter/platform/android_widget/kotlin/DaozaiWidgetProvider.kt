package __APP_PACKAGE__.widget

import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.Context
import android.widget.RemoteViews
import __APP_PACKAGE__.R
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Daozai 主屏小组件。
 *
 * 显示当前时间/日期 + 待办计数，点击拉起完整 App。
 * 包名占位符 __APP_PACKAGE__ 由 ci/setup_platform.sh 替换为 Flutter 实际 applicationId。
 * 待办数据经 shared_preferences 从 Flutter 侧写入，本 Provider 读取刷新。
 */
class DaozaiWidgetProvider : AppWidgetProvider() {

    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        for (id in ids) {
            val views = RemoteViews(context.packageName, R.layout.daozai_widget)
            val now = Date()
            views.setTextViewText(R.id.widget_clock, SimpleDateFormat("HH:mm", Locale.getDefault()).format(now))
            views.setTextViewText(R.id.widget_date, SimpleDateFormat("M月d日 EEEE", Locale.CHINA).format(now))

            // 读取 Flutter 侧缓存（shared_preferences 的 key 前缀为 flutter.）。
            val prefs = context.getSharedPreferences("FlutterSharedPreferences", Context.MODE_PRIVATE)
            val todoJson = prefs.getString("flutter.daozai.todos", null)
            val todoCount = countTodos(todoJson)
            views.setTextViewText(R.id.widget_todo, "待办 $todoCount 项")

            // 点击小组件拉起 App。
            val intent = context.packageManager.getLaunchIntentForPackage(context.packageName)
            if (intent != null) {
                val pending = android.app.PendingIntent.getActivity(
                    context, 0, intent,
                    android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE
                )
                views.setOnClickPendingIntent(R.id.widget_root, pending)
            }
            manager.updateAppWidget(id, views)
        }
    }

    private fun countTodos(json: String?): Int {
        if (json.isNullOrEmpty()) return 0
        return try {
            val arr = org.json.JSONArray(json)
            // 只统计未完成的待办（已勾选的不计入「待办 N 项」）。
            var n = 0
            for (i in 0 until arr.length()) {
                val obj = arr.optJSONObject(i)
                if (obj != null && !obj.optBoolean("done", false)) n++
            }
            n
        } catch (e: Exception) {
            0
        }
    }
}