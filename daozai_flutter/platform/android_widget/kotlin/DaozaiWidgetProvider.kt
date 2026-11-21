package com.daozai.flutter.widget

import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.Context
import android.widget.RemoteViews
import com.daozai.flutter.R
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 岛仔主屏小组件。
 *
 * 骨架阶段：显示当前时间/日期，点击拉起完整 App。
 * 天气与待办数据后续经 home_widget 插件或 MethodChannel 从 Flutter 侧写入
 * SharedPreferences，再由本 Provider 读取刷新（见 README「安卓小组件」）。
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
            org.json.JSONArray(json).length()
        } catch (e: Exception) {
            0
        }
    }
}