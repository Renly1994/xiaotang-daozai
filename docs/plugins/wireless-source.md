# 原生无线设备来源（实验宿主扩展）

这是基于 v0.6.71 的通用 SDK 扩展提案及本地验收实现，**尚未被官方 v0.6.71 发布版接受**。普通 v0.6.71 会因未知 `wireless.devices` 能力而拒绝安装。插件投稿应关联宿主扩展，等待维护者合入和发布后再上架，不得仅填写“支持官方 0.6.71”。

## 清单与授权

```json
{
  "id":"example.external-battery", "name":"外部电量来源", "author":"示例",
  "version":"1.0.0", "api":1, "runtime":"js", "entry":"main.js",
  "owner":"wireless", "cover":"cover.png", "dataVersion":1,
  "capabilities":["wireless.devices","timer"], "refreshSeconds":60,
  "permissions":{"source.read":"loopback"}, "settings":[]
}
```

`source.read` 必须为 `file` 或 `loopback`，沿用用户选择普通本地文件或授权回环 GET 接口的流程。HTTPS、任意文件访问、运行命令、插件自行启动进程和 HID 权限均未加入。一个插件只有一个授权来源；宿主读取上限、超时、取消及撤权规则保持不变。

该能力只可与 `timer`、`settings`、`storage` 一起声明，不同时注册独立状态页或插件常驻位。设备显示和常驻选择由原生无线设备页统一负责。

## 脚本输出

`refresh(input)` 在 `input.source` 接收 `{status:"unconfigured"}`、`{status:"offline"}` 或 `{status:"ready",text:"…"}`。`input.now` 为宿主提供的 Unix 毫秒数；开发者工具 `--input` 的模拟 `now` 会替换它。

返回 `{wireless:[…]}`，数组最多 64 条，每条字段如下。示例数值是合成格式样例，不是设备实测。

```js
function refresh(i) {
  if (i.source.status !== "ready") return {wireless: []};
  return {wireless: JSON.parse(i.source.text).devices};
}
```

| 字段 | 含义与边界 |
|---|---|
| `id` | 来源内稳定逻辑标识，1–128 字，不能含控制字符，数组内唯一。宿主在前面加入插件 ID 命名空间 |
| `name` | 设备型号，1–80 字，无控制字符 |
| `kind` | `mouse`、`keyboard`、`gamepad` 或 `headset` |
| `vendorId`、`productId` | 1–65535 的整数 USB 标识 |
| `usbInstance` | 可选的精确 USB 物理实例标识，最多 512 字。宿主核对实际枚举，插件不能声明不存在的接收器为在线 |
| `percent` | 可空整数 0–100，未知保持 `null`；有数值必须带测量来源时间 |
| `charging` | `true`、`false` 或 `null`；非空时必须带充电来源时间 |
| `approximate` | 默认 false，档位换算等估算数值需为 true |
| `online`、`fresh` | 默认 false，显式旧值不会被宿主提升为实时。online 需要连接来源时间；fresh 需要 online 和有效电量 |
| `measuredAtUtc` | 电量的真实 RFC3339 来源时间，保留原值 |
| `connectionCheckedAtUtc` | 连接的真实来源时间，不能用电量时间替代独立的后续断开记录 |
| `chargingCheckedAtUtc` | 充电状态的真实来源时间；无证据时充电应为 null |
| `freshForSeconds` | 30–180 秒，默认 90。不同来源可解释地设定较长周期，宿主持续按真实时间计算年龄 |

未来超过 5 秒、格式错误、未知字段、无效比例、重复标识和超量条目会使本轮结果失效。重新查询、写入新缓存文件和读取旧文件都不能重置来源时间。当前过期的条目可作为明确缓存保留，由原生侧执行年龄及连接检查；超过原生历史保留期后清空百分比。

## 生命周期与原生合并

插件启用、原生无线设备模块启用、来源已授权时才运行。关闭模块、停用或卸载插件、撤销授权、读取失败和本轮脚本失败都会撤下该来源结果，并丢弃在途的迟到结果。重启后重新读取来源，不将旧设备输出持久化为实时结果；用户常驻选择按稳定 ID 保留。

宿主合并前核对当前 USB 物理枚举，分配 `plugin:<插件ID>:<逻辑ID>` 标识；真实原生读数优先。来源不包含硬件扫描或设备控制动作，采集端的进程、缓存和协议生命周期由采集端自己负责。

公开投稿材料应保留型号及必要 VID/PID，去除本机路径、序列号、MAC、持久设备 ID、账户和未脱敏数据目录。异常边界测试与真实设备读数应分别说明。
