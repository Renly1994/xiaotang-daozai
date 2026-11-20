# Agent UI Map 调试输出契约 v1（2026-11-20）

> 目的：让测试脚本不再「截图 + OCR 找字」，而是直接读岛仔自己报的控件矩形与状态，坐标点击。
> 只影响调试/测试路径：**未设置环境变量时零开销、不落盘、不影响生产行为**。

## 开关

| 环境变量 | 含义 |
|---|---|
| `DAOZAI_UI_MAP` | 输出文件路径（或目录，则写 `<dir>/ui-map.json`）；未设置 = 关闭 |

写法要求：

- 原子写（`xxx.tmp` → rename），UTF-8 无 BOM；
- 节流：两次写之间 ≥ 50 ms，或状态变化时立即写；
- 只写**当前可见的那一屏**（岛体 + 当前卡片/结果层），不含整页内容。

## 坐标

- `nodes[].rect` 用 **canvas 逻辑坐标** `[x, y, w, h]`，原点 = canvas 左上角，y 向下；
- 测试脚本按 `屏幕坐标 = 窗口左上 + rect * scale` 换算（窗口矩形用 Win32 `GetWindowRect` 取，scale 见字段）；
- 不提供屏幕绝对坐标，避免窗口移动时地图失效。

## JSON

```json
{
  "schema": 1,
  "t": 12.345,
  "scale": 1.4,
  "canvas": { "w": 960.0, "h": 520.0 },
  "state": {
    "panel": true,
    "details": true,
    "notice": "agent",
    "queue": 1,
    "card": {
      "kind": "question",
      "id": "e2e-session-1",
      "details": true,
      "index": 0,
      "count": 2,
      "submitting": false,
      "error": null
    },
    "result": { "open": true, "hint": false, "title": "本轮回复", "scroll": 0.0, "maxScroll": 512.0 }
  },
  "nodes": [
    { "id": "island.bar", "rect": [0, 0, 640, 44] },
    { "id": "card", "rect": [80, 44, 419, 372] },
    { "id": "chip.jump", "rect": [..], "label": "切换到客户端", "enabled": true },
    { "id": "btn.allow", "rect": [..], "label": "仅允许这一次", "enabled": true }
  ]
}
```

- `state.notice`：`none` / `agent`（当前是否有 Agent 卡片占位）；
- `state.card.kind`：`approval` / `question` / `done` / `null`；
- `state.result`：结果层（含「查看回复」说明层）打开时才有值，`hint=true` 表示本机说明层；
- `nodes[].enabled` 缺省视为 true；`label` 是屏幕上的文案（用于断言/证据）。

## 节点 ID 约定（测试脚本按此取坐标）

| id | 出现场景 | 说明 |
|---|---|---|
| `island.bar` | 总是 | 岛体条 |
| `card` | 有卡片时 | 卡片外框 |
| `chip.jump` | 卡片/收起提示卡 | 切换到客户端 |
| `chip.collapse` | 卡片头部 | 收起 |
| `btn.return` | 审批/提问 footer | 回客户端 |
| `btn.deny` | 审批 | 拒绝 |
| `btn.allow` | 审批 | 仅允许这一次 |
| `body.operation` | 审批 | 操作内容区 |
| `option.<i>` | 提问 | 第 i 个选项（从 0 开始） |
| `input.custom` | 提问 | 自定义回答输入框 |
| `btn.prev` / `btn.next` | 多题提问 | 上一题 / 下一题 |
| `btn.submit` | 提问 footer | 检查答案 / 提交回答 |
| `btn.confirm` | 提问审阅页 | 确认提交 |
| `btn.view` | 完成卡 | 查看回复 |
| `btn.close` | 完成卡 footer / 结果层 footer | 关闭 |
| `btn.dismiss` | 完成卡轻提示 | 轻提示上的关闭 |
| `layer.result` | 结果层 | 结果层外框 |
| `btn.copy` | 结果层（非说明层） | 复制内容 |
| `body.result` | 结果层 | 正文区（用于滚动） |

## 验收

1. `DAOZAI_UI_MAP` 未设置时：无任何文件写入、无额外锁竞争（单测覆盖开关关闭路径）；
2. 设置后：审批卡 / 提问卡 / 完成卡 / 结果层 四类视图都能导出 `card` + 至少一个按钮节点，`rect` 与把 `label` 点下去后实际发生的动作一致；
3. 卡片打开/收起、结果层打开/关闭时 `state` 与 `nodes` 在 1 帧内（≤50 ms）更新。
