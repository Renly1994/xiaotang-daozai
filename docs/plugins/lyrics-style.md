# 歌词样式扩展（lyrics.style v1）

无脚本资源插件，启用后扩展原生音乐与岛上歌词的卡拉 OK 逐字变色样式。未启用或卸载后恢复原本纯白歌词。只能启用一个歌词样式插件。

维护者负责清单、样式配置、设置项、封面和插件版本，不需要提交脚本。媒体进度读取、KRC/YRC 逐字解析、无损结构化缓存、双向文字映射、互补裁剪染色、浅色皮肤可读性适配以及「设置 → 音乐」中的可视化色块与动态预览卡片均由本体负责，插件不读取播放或本机数据。

## plugin.json

```json
{
  "id": "swidan123.karaoke-lyrics",
  "name": "逐字变色歌词",
  "author": "swidan123",
  "version": "1.0.1",
  "api": 1,
  "runtime": "assets",
  "entry": "lyrics-style.json",
  "cover": "cover.png",
  "owner": "music",
  "capabilities": ["lyrics.style", "settings"],
  "dataVersion": 1,
  "permissions": {},
  "settings": [
    {"key": "island_enabled", "label": "岛上歌词逐字变色", "type": "switch", "default": "true"},
    {"key": "expanded_enabled", "label": "展开歌词逐字变色", "type": "switch", "default": "true"},
    {"key": "color", "label": "已唱颜色", "type": "text", "default": "rainbow"},
    {"key": "custom_color", "label": "自定义色号", "type": "text", "default": "#38BDF8"}
  ]
}
```

## lyrics-style.json

```json
{
  "version": 1,
  "island": true,
  "expanded": true,
  "defaultColor": "rainbow",
  "defaultCustomColor": "#38BDF8",
  "islandSetting": "island_enabled",
  "expandedSetting": "expanded_enabled",
  "colorSetting": "color",
  "customColorSetting": "custom_color"
}
```
