# 岛仔插件 SDK v1

## 1. 插件如何工作

SDK 是岛仔提供给插件作者的一组公开约定。宿主指运行插件的岛仔。你提供一份清单和几段函数，岛仔负责调用、画界面和保存结果。

第一版采用**每次调用都从头运行**的写法。函数只根据输入计算并返回结果，需要记住的内容放进返回值的 `storage`。两次调用之间，脚本变量会消失。宿主负责安排刷新、读取已授权的数据源和接收用户点击。当前 Windows 测试版可运行 QuickJS 待办卡片、日程日期标注、AI 额度来源与独立状态页。[运行环境实测](runtime-spike.md) · [本地安装](local-install.md)

## 2. 我能写哪些功能

先试[发薪倒计时](../../spike/examples/payday/plugin.json)和它的[完整入口脚本](../../spike/examples/payday/main.js)。复制整个目录，修改清单中的 ID（识别插件的固定名称）和显示名称，就能安装第二份。`plugin.json` 是插件的清单。JSON 用字段名和值来记录设置、刷新间隔和权限。

当前宿主按全局函数名调用 `refresh(input)`。输入是 `{settings, today, storage}`，日期格式为 YYYY-MM-DD，首次调用的存储值为 `null`。待办卡片返回 `{card, storage}`，其中 card 为文字，storage 为本次完整的私有数据。下面这段可以直接替换样例的 main.js。

```js
function refresh({ settings, today }) {
    const days = Math.round((Date.parse(settings.target) - Date.parse(today)) / 86400000);
    return { card: settings.title + " · " + days + " 天", storage: { lastDays: days } };
}
```

| 能力与进度 | 作者怎么写 | 用户在哪里用 |
| --- | --- | --- |
| 文本、日期设置 · 已实现 | 清单的 settings 列出字段，在上例读取 settings.title 和 settings.target | 归属模块的插件设置 |
| 材质数字参数 · 已实现 | 清单声明 `type: "number"`、`min`、`max`、`step` 和字符串形式的 `default`，可选 `unit` 和 `precision`。着色器按清单顺序用 `param(index)` 取值，最多 32 项 | 「设置 → 外观 → 材质」，拖动滑块实时应用 |
| 脚本插件设置 · 实验 | text、date、switch、choice、number、time、weekdays、timezone、list。默认值和保存值均为字符串 | 所属设置页 |
| 文字卡、私有存储 · 已实现 | 上例返回 card 和 storage。首次的 storage 为 null，之后可用 `storage?.lastDays` 读回天数 | `owner: "tasks"` 显示在待办页 |
| 日期标注 · 已实现 | 声明 `calendar.marker`，`refresh` 返回 `marker`，空字表示当天不标注。旧版 `owner: "calendar"` 加 `card` 的写法继续有效，`card` 即标注 | 展开「日程 → 日历」查看电脑当天日期下方的标注 |
| 一个插件多处输出 · 测试版 | 在 capabilities 里同时列出几种输出，例如 `["card","calendar.marker","timer"]`，一次返回 `{card, marker}`。同时有多种输出时，某项返回空值表示这次不显示 | 每处输出只跟着自己所在模块的开关，关日程只撤日历标注；关掉插件归属的模块则整个插件暂停 |
| 周期刷新 · 已实现 | 两个样例在清单声明 `"refreshSeconds":60` 和 timer 能力。到期由岛仔调用 refresh | 卡片按期更新 |
| 岛上常驻位 · 实验 | 声明 `island.pinned`，通过 `refresh` 返回 `pinned:{text:"今日金价",value:"850 元/克",icon:"coin"}` | 收起时各插件一起显示，用户可调整顺序 |
| 结构化状态卡和独立详情页 · 测试版 | 清单使用 `"owner":"status"` 和 `"capabilities":["status.page","timer"]`，`refresh` 返回 `status`。可用 `statusLayout` 选择组件、排序和样式。 | 在岛上翻页查看，在「设置 → 页面管理 → 状态详情」单独启停 |
| [邮箱视图扩展](mail-view.md) · 测试版 | 声明无脚本、无权限的 mail.view 资源包，扩展原生邮箱的多账号视图。 | 启用后在原生邮箱页使用，不注册独立状态页 |
| [歌词样式扩展](lyrics-style.md) · 测试版 | 声明无脚本、无权限的 lyrics.style 资源包，扩展原生音乐与岛上歌词的卡拉 OK 逐字变色样式。 | 启用后在「设置 → 音乐」页配置，停用或卸载即恢复原生纯白歌词 |
| 状态页组件与主题令牌 · 实验 | 在状态页布局中添加列表、迷你折线、多行说明和标签，可设强调色、强调程度及岛仔内置图标 | 在状态详情页查看 |
| 自绘画布 · 实验 | 同时声明 `status.page` 和 `status.canvas`，脚本返回画布的图元与样式 | 可填满状态详情页或与标准组件混排 |
| 实时状态组件与敏感值 · 实验 | 布局选择 `counter`、`clock`、`countdown`，由岛仔计算实时读数。金额等数据须标记 `sensitive:true`。 | 在状态详情页查看，点击暂时显示敏感值 |
| 状态详情交互 · 实验 | 同时声明 `action` 和 `status.page`。可使用按钮、文本表单、可点的列表项，以及宿主的打开链接和复制动作 | 在状态详情输入内容或点击操作 |
| 可编辑独立模块 · 待宿主实现 | 清单声明 module 与按钮 `actions:["save"]`。示意 `function action(i){return {page:{kind:"detail",text:i.form.text},storage:{note:i.form.text}}}` | 岛上功能栏查看、编辑，侧栏调偏好，页面管理启停 |
| 日程来源 · 待宿主实现 | `function refresh(i){return {calendar:[{id:"anniversary",title:i.settings.title,date:i.settings.target}]}}` | 日程里的来源，按来源和条目 ID 更新 |
| 歌词来源 · 待宿主实现 | `function refresh(i){return {lyrics:{trackId:i.track.id,lines:i.source.lines}}}`。换歌后宿主只接收当前曲目的结果 | 原音乐页 |
| AI 额度来源 · 测试版 | 清单使用 `"owner":"quota"` 和 `"capabilities":["quota.source","timer"]`，`refresh` 返回 `quota`。 | 原 AI 额度面板中按插件名称显示来源页签 |
| 专注事件 · 实验 | 声明 event，清单顶层写 "events":["focus.finished"]。专注计时完成时调用 event(input) | 启用插件后完成一轮专注 |
| 一次性提醒 · Windows 实验 | 声明 notice，脚本返回 notice 对象 | 岛上的通知与未读，遵循系统内的通知和正文显示开关 |
| 只读数据源 · Windows 测试版 | 清单通过 `permissions.source.read` 声明 `file`、`loopback` 或 `https`。宿主将已授权数据传给 `refresh` 的 `source` 输入。 | 「设置 → 插件 → 已安装 → 授权数据源」 |
| 皮肤 · 测试版 | `runtime: "assets"`、`owner: "appearance"`、`appearance.skin`，入口 `theme.json` | 设置 → 外观 → 皮肤 |
| 功能图标 · 测试版 | `runtime: "assets"`、`owner: "appearance"`、`appearance.icon`，入口 `icons.json` | 设置 → 外观 → 功能图标，替换设置侧栏及功能标题卡图标 |
| 吉祥物表情 · 测试版 | `runtime: "assets"`、`owner: "appearance"`、`appearance.pet`，入口 `expressions.json` | 设置 → 外观 → 吉祥物表情，沿用原动作的触发时机与帧速 |
| 画布字体 · 测试版 | `runtime: "assets"`、`owner: "appearance"`、`appearance.font`，入口 `fonts.json` | 在已安装插件中启用，画布文字按名称引用 |
| 底部快捷切换 · 已实现 | 声明 `runtime: "assets"`、`owner: "pages"` 和 `navigation.shortcuts`，在 `shortcuts.json` 配置选中配色 | 展开岛仔，点击底部图标直达已启用页面。设置归属「页面管理」 |
| GPU 材质 · 已实现 | 清单用 `runtime: "material"`、`owner: "appearance"`、`appearance.material` 能力和 `screen.backdrop` 权限。WGSL 入口实现 `material()`，沿用[材质接口](gpu-material.md) | 「设置 → 外观 → 材质」，同一时间选用一个材质，需开启 Direct3D 12 兼容渲染 |

action 接收按钮 ID 和表单值，event 接收事件名和事件数据。两个入口沿用 refresh 的输出与私有存储格式。

## 3. 用户去哪里用，去哪里调

岛仔用一张能力表核对清单（[能力表](../../spike/capabilities.rs)）。每种能力写明需要的运行环境、落在哪里，以及能否和别的能力同时声明。`owner` 只决定插件设置放在哪个设置页，以及哪个模块关掉时整个插件暂停。脚本插件的归属可选 tasks、calendar、quota、status，并且至少要带上这个归属自己的输出：tasks 带 card，calendar 带 card 或 calendar.marker，quota 带 quota.source，status 带 status.page。材质、皮肤、图标、表情、快捷切换和无线设备兼容包各自独占一个包。外观资源包只声明一种能力，`permissions` 填 `{}`，`settings` 填 `[]`。

外观资源包由 `plugin.json`、入口文件和 `cover.png` 组成，本地开发目录也可带说明文件。皮肤、图标和表情的集市 ZIP 上限为 832 KiB，字体包另有文件限制。各类入口使用 UTF-8 JSON。

- 画布字体包的 `fonts.json` 格式为 `{"fonts":[{"name":"站酷快乐体","file":"ZCOOLKuaiLe-Regular.ttf"}]}`。可列出 1 至 4 个名称和文件各不相同的字型，支持 TTF 与 OTF，单个最多 8 MiB，整包最多 16 MiB。包根目录须有 `LICENSE` 或 `OFL.txt`。启用后，画布文字可用 `font:"站酷快乐体"`，宿主以 `plugin:站酷快乐体` 注册。缺字、停用或卸载时沿用岛仔字体。 [站酷快乐体样例](../../spike/examples/font-zh/plugin.json) · [Google Fonts 字体许可](https://github.com/google/fonts/blob/main/ofl/zcoolkuaile/OFL.txt)

- 皮肤包入口最多 4 KiB，例如 `{"base":"cream","colors":{"paper":"#F4F8F6","ink":"#20382F","accent":"#248161"}}`。经典 `classic` 和奶油 `cream` 开放 `paper`（底色）、`ink`（文字）和 `accent`（强调色）。毛玻璃 `glass_light`、`glass_dark` 和悬浮云 `fluid` 开放 `accent`，保留原来的透明度和纹理。至少提供一个 `#RRGGBB` 颜色。缺少的颜色沿用基础皮肤。可参考[青瓷皮肤样例](../../spike/examples/celadon-skin/plugin.json)。
- 图标包入口最多 128 KiB，格式为 `{"icons":{"music":"<svg …>…</svg>"}}`。功能页键可用 `music`、`focus`、`health`、`shelf`、`tasks`、`mail`、`quota`、`calendar`、`weather`、`performance`、`wireless`。每个 SVG 最多 8 KiB，声明 SVG 命名空间与 `viewBox`，使用路径、基础形状和分组，可用十六进制颜色、`none` 或 `currentColor`。禁止脚本、样式表、外部资源和 `foreignObject`。
- 表情包入口最多 768 KiB，格式为 `{"expressions":{"wink":"PNG 的 Base64 字符串"}}`。可替换 `wink`、`smug`、`blush`、`delighted`、`side-glance`、`praised`、`heart`、`giggle`、`shy-peek`、`scan`、`peek`、`search`。每张 PNG 为 144×1440，从上到下排列 16 帧，每帧 144×90，最多 64 KiB。缺少的图标或表情沿用内置素材。

用户习惯去原功能完成的事，可以往原设置页加一项。皮肤进入外观选择器，歌词来源进入音乐设置。只需要看一眼的内容可以做卡片，少量参数仍放在所属页。

用户要反复编辑便签、管理纪念日，或专门查看打印进度、温度和告警时，可以新增独立模块。作者选择展示内容与样式，岛仔负责字体、缩放和点击。正文和一次性输入在功能页编辑，默认排序等偏好在模块设置页修改。

状态详情页通过 `owner: "status"` 和 `status.page` 接入，按照插件 ID 找到页面和独立开关。岛上的齿轮与「已安装」里的页面路径都进入页面管理。可编辑独立模块的设计使用 `owner: "self"` 和 `module`。

每个插件只有一个主要设置归属。「已安装」每行统一提供路径（前往设置）、开关和「···」菜单（查看详情、反馈问题、卸载）。列表按设置归属分组，材质组同时只开一个。页面管理控制模块运行。设置页复用岛仔已有的开关、选择、数字、文本行和功能标题卡，保留必要设置与关键提醒，合并重复提示，内容多时才收起低频选项。

插件提供的设置项在所属设置页使用统一样式。标题旁显示「插件」小标，插件开关开启时用紫色，关闭时用灰色。复用宿主现有的小标和开关组件。页面快捷切换等显示偏好开关也使用该样式，各自保存对应的显示选择。

### 岛上常驻位（实验）

插件保留原有归属和对应输出能力，并在清单增加 `island.pinned`。`pinned.text` 最长 12 字，`value` 可用数字、最多 24 字的文字、`{counter:{…}}` 或 `{countdown:{…}}`。实时参数与状态页共用，由宿主按需重绘。`icon` 可选 `clock`、`chart`、`coin`、`calendar`、`cup`、`cloud`。通过 `pinned.sensitive:true` 隐藏值，计数器自身的 `sensitive:true` 也会生效。

同时声明 `status.canvas` 后，也可返回 `pinned.canvas` 对象或对象数组。同一插件有多张常驻画布时，每八秒切换一张。

在插件所属设置页开启「在岛上常驻显示」，再到「设置 › 页面管理 › 岛上显示」调整顺序。收起时各插件一起显示，上下停靠时横向并排，左右停靠时纵向排列。横向岛体按内容加宽，受显示空间限制时由岛体统一适配。通知弹窗（含接入为通知的来电）和岛上歌词优先显示。展开、锁屏、全屏隐藏或极简模式时让出位置。关闭插件或其所属模块会立即撤下内容。

## 4. 关闭后会发生什么

「已安装」的开关保存用户对插件的启用选择，只表示插件自己，不随模块状态变色。「页面管理」独立保存所属模块的启用选择，独立模块也遵循这条规则。两个开关都开启时，插件才有运行资格。安装后默认停用。

| 插件总开关 | 所属模块开关 | 「已安装」显示 | 设置、岛体与后台 |
| --- | --- | --- | --- |
| 关 | 任意 | 灰色开关，封面去色淡化 | 收起插件设置、岛体内容和专属功能入口，停止发起新的定时调用，保留数据 |
| 开 | 关 | 紫色开关，路径变成琥珀色「页面管理 › …」 | 收起插件输出和设置，暂停定时调用，保留原启用选择，点路径前往页面管理 |
| 开 | 开 | 紫色开关，路径「设置 › …」 | 恢复内容、设置入口和定时调用，使用已保存的数据 |

两个开关各自持久化。暂停模块后，重新启用插件总开关时模块仍暂停。先停用插件再重开所属模块时，插件仍停用。显示位置开关只影响对应的显示位置。

| 用户操作 | 应看到的结果 |
| --- | --- |
| 在「已安装」关掉今日农历的开关 | 日历里的农历和节气标注消失，日程页仍可使用 |
| 关日程，再开日程 | 关闭时暂停农历插件。重新打开日程后，已启用的农历插件恢复显示 |
| 纪念日同时在日历和常驻栏展示 | 关日程只撤日历记录。关纪念日模块时撤下功能入口、固定倒数及日历记录，「已安装」保留总开关，路径提示去页面管理。两项开关都开启时找回原文、日期和固定选择 |
| 奶油皮肤换成插件皮肤，期间把胶囊改成刘海，再停用或卸载皮肤包 | 回到奶油＋刘海，保留用户设置的停靠位置。皮肤包重新启用后可再次到外观选择 |
| 停用或卸载正在使用的图标包或表情包 | 立即换回对应的内置素材 |
| 停用后重启，或卸载 | 停用立即撤下输出，宿主停止发起新调用。已经开始的一次计算即使稍后返回也丢弃结果。重启保持停用，卸载可选择保留或删除数据 |

修改设置、关上级模块或重新安装时，同样作废此前发出的计算。插件重新启用后按当前设置重新计算。单选来源由岛仔记住，当前来源不可用时回到最近一次可用的选择。未获授权、执行出错或超时的调用，其结果一律作废。

## 5. 谁可以读什么

权限说明用户允许岛仔替这个插件做哪些事。安装前看作者、归属和权限摘要，首次启用确认具体用途，更新时新增的权限单独确认。撤权后岛仔停止取数并丢弃旧请求的结果。

JS 卡片可用空权限，或申请一个本地只读数据源。材质插件首次启用时单独确认 `screen.backdrop` 权限，岛背后的像素只在本机用于材质绘制，着色器可采样底片纹理，像素不保存、不上传。

### 只读数据源

待办、日程、额度来源和状态详情插件可以申请一类数据源。在「设置 → 插件 → 已安装」授权所选文件、本机 HTTP 地址或清单声明的 HTTPS 来源。授权可随时撤销，卸载会清除授权。

- `file` 读取系统选择器选中的一个 UTF-8 文件，拒绝网络盘、目录和重解析路径。数据文件可以在原路径更新。
- `loopback` 对用户确认的完整地址做 GET，例如 `http://127.0.0.1:8080/status`。支持 `127.0.0.1`、`[::1]` 和 `localhost`，直接连接回环地址，设置 2 秒请求预算，拒绝重定向、URL 账号密码和非回环地址。
- `file` 和 `loopback` 的读取上限均为 128 KiB。
- `https` · 实验。清单在 `network.hosts` 列出 1 至 4 个完整域名，由用户一次授权 `network.sources` 声明的 1 至 3 个 GET 来源。每个来源使用唯一 `key`，路径和查询串可以用 `{settings.设置键}` 引用清单中声明的设置值，宿主按 UTF-8 百分号编码。旧清单仍可使用 `network.url` 预填单一地址。
- HTTPS 每次请求都按白名单核对地址，只连接解析出的公网地址，使用 443 端口。每个来源禁用代理和重定向，超时 10 秒，响应最多 256 KiB UTF-8。
- 脚本收到状态和响应文本。路径、地址、密钥及文件和网络操作留在宿主。

需要密钥的清单可声明 `credential`。宿主将密钥保存在 Windows 凭据管理器，按声明的请求头和前缀向每个来源发送。撤销授权或卸载会删除密钥。更新时改动域名白名单、来源或凭据配置，需要重新确认授权。以下是使用 GoldAPI.io 的清单片段。

```json
{
  "permissions": {"source.read": "https"},
  "network": {
    "hosts": ["www.goldapi.io"],
    "sources": [{"key": "gold", "url": "https://www.goldapi.io/api/price/XAU/CNY"}],
    "credential": {
      "label": "GoldAPI.io API Key",
      "header": "x-access-token",
      "prefix": ""
    },
    "minRefreshSeconds": 300
  },
  "attribution": {"label": "GoldAPI.io", "url": "https://www.goldapi.io/price/XAU/CNY"}
}
```

[国际现货金价样例](../../spike/examples/gold-price)可直接试装，演示迷你折线和阈值提醒。XAUS 的 `/api/v1/spot?currency=CNY&unit=gram` 可返回人民币每克的国际现货参考报价。XAUS 要求至少缓存 60 秒，高用量商业集成应按其公平使用说明联系服务方。[XAUS API 公平使用说明](https://xaus.com/api/#fair-use)

样例把最近 48 个新报价存入 `storage.points` 并绘制趋势。价格向上或向下穿越阈值时，岛仔显示一次性提醒，报价时刻用于提醒去重。独立验证本步 HTTPS 时可试装[仓库动态样例](../../spike/examples/https-repository)。

`network.sources` 按清单中的 key 向脚本提供 `sources`，例如 `sources.gold`。文件、本机 HTTP 和旧版单一 HTTPS 仍使用 `source`。每个来源未授权为 `{status:"unconfigured"}`，无法读取为 `{status:"offline"}`，读取成功为 `{status:"ready",text:"…"}`。校验和打包时传入 `unconfigured`。单个来源失败后独立退避，间隔依次为正常值的 1、2、4、8 倍，退避上限为 30 分钟，并遵守用户设置的正常间隔。成功一次后恢复正常周期。

`timer` 按 `refreshSeconds` 调度。声明 `minRefreshSeconds` 后，所属插件设置页提供「刷新间隔」，可在该值至 86400 秒之间选择，未设置时使用 `refreshSeconds` 下限校正后的值。清单可选填 `attribution`，宿主在状态页和已安装详情显示来源与最近成功取数时间。

```js
function refresh({ source }) {
  if (source.status === "unconfigured") return { card: "请授权数据源" };
  if (source.status === "offline") return { card: "数据源暂不可用" };
  return { card: source.text.trim().slice(0, 80) || "暂无数据" };
}
```

停用插件、关闭归属页面、撤权或更换授权会停止旧来源的后续读取，并丢弃已经发出的读取结果。数据源失联时只影响该插件的输入，下一次刷新可以恢复。

### AI 额度来源

在 JS 清单中声明 `"owner":"quota"` 和 `"capabilities":["quota.source","timer"]`。需要取数时，加上上一节的 `source.read` 权限。插件的 `refresh` 返回 `{quota: ...}`，额度面板用插件清单中的名称展示来源。

```js
function refresh({ source }) {
  if (source.status !== "ready") return { quota: null };
  const { remaining, total, resetAt } = JSON.parse(source.text);
  return { quota: { windows: [
    { label: "当前周期", remaining, limit: total, unit: "积分", reset_at: resetAt }
  ] } };
}
```

- `windows` 可提供 1 至 3 项，每项填写 `label`。百分比使用 `remaining_pct`，范围为 0 到 100。真实数值使用非负的 `remaining` 和 `unit`，可另填正数 `limit`，保留原单位。
- 每项可填 `period_secs` 和 `reset_at`，分别是周期秒数和重置 Unix 时间戳。整份额度可选 `expires_at`，用于显示到期 Unix 时间戳。到期与重置各自独立。
- 可返回 `quota: null` 撤下当前读数。申请取数权限的插件，在未授权或读取失败时由宿主管理撤下；其他来源的数据和选中设置可继续使用。暂停额度页或停用插件时，该来源会撤下。面板手动刷新会同时触发已启用的额度插件。
- 额度输出保留在本次运行中。若插件声明了 `storage`，脚本可额外返回自己的私有存储。打包校验时源状态为 `unconfigured`，入口应能处理该状态。

### 结构化状态卡和详情页

JS 清单使用 `"owner":"status"`，能力列出 `status.page`。可选的 `timer`、`settings` 和 `storage` 沿用现有约定。读取文件或本机接口时使用同一套 `source.read` 授权。宿主会用清单的显示名称和固定插件 ID 注册详情页，无需声明页面编号。

清单可用 `titleAccent` 指定 `name` 中要着色的末尾文字。例如 `"name":"现货金价","titleAccent":"金价"`。颜色取 `statusLayout.style.accent`，省略时用当前皮肤的默认强调色。插件列表仍使用完整名称。

独立状态页插件可在 `plugin.json` 中声明 `navigationIcon`。宿主在设置侧栏和岛上快捷导航绘制同一轮廓。侧栏使用 `background` 的底色和白色描边，快捷导航沿用当前的选中与悬停色。省略此字段时显示宿主图标。

```json
{
  "navigationIcon": {
    "path": "M5 6H19V18H5Z M8 10H16 M8 14H13",
    "background": "#587CA8"
  }
}
```

`path` 使用 24×24 坐标，端点和控制点都在 0 至 24 之间。支持 `M/L/H/V/Q/C/Z` 及其小写相对命令，必须包含可绘制的线段或曲线，最多 64 条命令和 1024 字节。`background` 使用 `#RRGGBB` 格式。

返回 `{status: ...}`，`note` 支持多行，其余文字字段填单行可显示文字。

| 字段 | 取值 |
| --- | --- |
| `state`、`label` | 必填。状态选 `running`、`paused`、`succeeded`、`error`、`offline` 或 `idle`，标签 1 至 40 字 |
| `summary`、`alert` | 可选的摘要与告警，各 1 至 120 字 |
| `progress_pct` | 可选，0 至 100 的数字 |
| `metrics` | 可选，最多 6 项。每项填 `label`（1 至 28 字）和 `value`（1 至 32 字），数值和单位可放在同一段文字，可用 `sensitive:true` 隐藏值 |
| `items` | 可选，最多 8 项。每项有 `label`（1 至 28 字），可选 `detail`（1 至 80 字）、`state`（同上）、`action`（第 1 步提供的点击动作 ID）和 `sensitive:true`（隐藏整项） |
| `series` | 可选，2 至 48 个有限数字，按数组顺序绘制。可同时提供 1 至 12 字的 `series_label` |
| `note` | 可选，1 至 300 字的多行说明，支持 LF 或 CRLF 换行 |
| `tags` | 可选，最多 4 个标签，每个 1 至 8 字 |
| `counter` | 跳动数字，每项填 `start_value`、`per_second`、`start_at`（Unix 秒）及 `decimals`（0 至 4）。可用 `until`（Unix 秒）在终点停住，可加 `prefix`、`suffix`（各最多 12 字）及 `sensitive:true` |
| `clock` | 时钟，每项填 `label`、`utc_offset_minutes`（–840 至 +840）及 `format`（`HH:mm` 或 `HH:mm:ss`） |
| `countdown` | 倒计时，每项填 `label`、`target_at`（Unix 秒）及 `format`（`d天 HH:mm` 或 `HH:mm:ss`）。分钟格式向上取整，到时显示 `done_label`，省略时显示「已结束」 |
| `observed_at`、`expires_at` | 可选的 Unix 秒，分别表示观察和过期时间。观察时间须早于或等于过期时间，到了过期时间会撤下旧读数 |

#### 自定义详情页布局

在 `owner: "status"` 的插件清单中添加 `statusLayout`。缺省时使用原有默认模板。布局从清单读取，`refresh` 仍按上表返回 `status`。下例在状态标题后显示横向进度条、摘要、指标和告警。

```json
{
  "statusLayout": {
    "components": [
      {"type": "state"},
      {"type": "progress", "variant": "bar"},
      {"type": "summary"},
      {"type": "metrics", "columns": 2},
      {"type": "alert"}
    ],
    "style": {"accent": "#5BADEF", "density": "compact", "surface": "card"}
  }
}
```

选择 1 至 10 个组件，数组顺序就是展示顺序。普通组件每种最多一个，三种实时组件各可重复 4 次，每次计入布局上限。实时数据接受单个对象或数组，布局中第几个同类组件对应数组的第几项。有数据的组件按顺序显示，卡片内容溢出时可以在卡片内滚动。

有 `canvas` 组件和 `action` 能力时，可用 `statusLayout.canvasFields` 把最多三个单行输入框放到画布内。例如 `[{"key":"note","rect":[4,207,130,22],"placeholder":"记一笔…","action":"add"}]`。坐标从画布左上角起算，使用逻辑像素整数。返回同名 `status.fields` 和对应的 `status.actions` 后，用户可直接在画布内录入，按回车提交。画布内的底色和边框由插件设置。锚点不可用时，仍可用布局中的标准输入框和按钮操作。

| 组件 `type` | 数据与选项 |
| --- | --- |
| `state` | 显示 `state` 和 `label` |
| `summary` | 显示 `summary` |
| `progress` | 显示 `progress_pct`。`variant` 可选 `ring`（默认，圆环）或 `bar`（横向进度条） |
| `metrics` | 显示 `metrics`。`columns` 可选 `1` 或 `2`（默认） |
| `list` | 显示 `items`，用颜色点表达每项的状态。每项的点击和复制需要 `action` 能力 |
| `chart` | 将 `series` 绘成迷你折线，包含网格、面积填充和末点 |
| `text` | 按行显示 `note`，超长部分用省略号提示 |
| `badge` | 显示 `tags` |
| `alert` | 显示 `alert` |
| `updated_at` | 显示 `observed_at` |
| `actions` | 显示 `actions` 中的按钮，需要 `action` 能力 |
| `fields` | 显示 `fields` 中的文本框，需要 `action` 能力 |
| `counter`、`clock`、`countdown` | 用对应的字段在宿主计算读数。按秒或分钟的边界重绘，脚本按原周期刷新 |
| `canvas` | 绘制返回值中与 `status` 同级的 `canvas`，需要 `status.canvas` 能力 |

`style` 可以省略。强调色 `accent` 使用六位 `#RRGGBB`，岛仔按实际底色校正到至少 3:1，省略时沿用岛仔默认状态色。间距 `density` 可选 `comfortable`（默认）或 `compact`。紧凑模式最多显示 3 项列表、2 行说明、3 个标签。背景 `surface` 可选 `card`（默认）或 `plain`。强调程度 `emphasis` 可选 `normal`（默认）或 `strong`，作用于状态标题和折线。`icon` 可将岛仔内置的 `battery`、`cup`、`clock`、`chart`、`cloud` 或 `calendar` 图标放到状态标题旁。

[打印机状态样例](../../spike/examples/printer-status/plugin.json) 通过模拟脚本演示列表、折线、说明、标签和主题令牌。[实时工资](../../spike/examples/live-pay/plugin.json) 和 [世界时钟](../../spike/examples/world-clock/plugin.json) 分别演示敏感实时金额、常驻上下班倒计时，以及最多四个可配置的城市时钟。世界时钟由宿主换算所选 IANA 时区。

敏感值默认打码，点击后显示 30 秒，再次点击可立即盖上。敏感计数参数更新后重新打码。锁屏、岛仔演示录制及开启「截图和录屏中隐藏岛仔」时始终打码。使用第三方录屏前可开启此设置。

返回 `{status:null}` 时显示等待状态。取数失败会撤下旧读数，在本页显示数据源暂不可用。来源正常返回 `state:"offline"` 时，按插件提供的设备离线状态展示。宿主按插件 ID 分开刷新和撤下状态。

全新的插件默认停用，首次启用后详情页默认出现。可在「页面管理 → 状态详情」保存这个页面自己的开关。关闭详情页后，岛体撤下对应页面和状态卡，后台停止读取该插件的数据源，正在进行的结果作废。再次启用按当前授权重新读取。

#### 自绘画布

JavaScript 插件在清单中同时声明 `status.page` 和 `status.canvas`。`refresh`、`action`、`event` 都可以返回 `canvas`。返回画布时，省略布局或只配置 `{"type":"canvas"}` 会让画布占满页内内容区。与标准组件混排时，在 `statusLayout` 中添加一次 `{"type":"canvas"}` 指定画布位置，并通过同一份结果的 `status` 为标准组件提供数据。

```js
function refresh() {
    return {
        canvas: {
            height: 64,
            nodes: [
                { rect: [0, 0, 387, 64], r: 10, fill: "$card" },
                { text: "从这里开始画", at: [16, 38], size: 16, fill: "$text" }
            ]
        }
    };
}
```

坐标单位为逻辑像素，原点在标题行下方的内容区左上角，宽 387。画布高度 `height` 为 40 至 360。宿主随渲染缩放绘制，静止画面复用缓存。

| 图元 | 字段 |
| --- | --- |
| 矩形 | `rect:[x,y,w,h]`，可用 `r` 指定统一圆角或按左上、右上、右下、左下给出四个圆角 |
| 圆 | `circle:[cx,cy,r]` |
| 弧 | `arc:{c:[cx,cy],r,start,sweep}`，角度从水平向右开始，顺时针为正 |
| 直线 | `line:[x1,y1,x2,y2]` |
| 折线 | `points:[[x,y],…]`，加 `area:y` 可填充曲线与水平线间的区域 |
| 路径 | `path:"M0 0 L10 10 Q12 4 18 6 Z"`，支持 M、L、H、V、Q、C、Z 及其小写相对命令 |
| 文字 | `text:"文字",at:[x,y],size:14`。`at` 为基线位置，可加 `weight`、`align`（left、center、right）、`max_width`、`lines`、`font` |
| 图片 | `image:"badge.svg",rect:[x,y,w,h]`，用 `fit` 选择 cover（裁切铺满）或 contain（完整显示） |
| 分组 | `group:[…]`，可加 `clip:[x,y,w,h]`、`translate:[x,y]`、`rotate`（度）、`scale` |

`fill` 和 `stroke` 可以用 `#RRGGBB`、`#RRGGBBAA` 或跟随皮肤的 `$accent`、`$text`、`$muted`、`$dim`、`$card`、`$track`、`$bg`。文字颜色也可以用 `color`。线性渐变写作 `{linear:[[x1,y1],[x2,y2]],stops:[[0,"#FFE58A"],[1,"#FFE58A00"]]}`，径向渐变将轴换成 `radial:[[cx,cy],r]`。渐变停靠位置按从 0 到 1 的顺序排列。线型用 `width` 设置线宽、`cap` 选择 round 或 butt、`dash:[实线长,间隔长]` 设置虚线。通用效果有 `opacity`（0 至 1）、`shadow`（sm、md、lg）和 `glow`（true 或一种颜色）。可用 `id` 为图元提供稳定名称。

本地安装和 ZIP 都支持用包内相对路径引用 PNG 和静态 SVG，例如 `images/badge.svg`。资源随包放置，直接在图元中填写路径即可。`/` 分隔路径层级，宿主拒绝绝对路径、含 `..` 的路径和符号链接。SVG 需要有效的 `viewBox`，使用路径、矩形、圆等静态矢量图形。

画布图片每张最大 1024 × 1024，文件合计最多 512 KiB。集市封面独立校验。封面尺寸合规且画布还有剩余图片额度时，可以用 `image:"cover.png"` 引用，并计入画布图片总量。

内置图标节点写作 `{"icon":"heart","rect":[0,0,22,22]}`，图标名与列表设置的 `icon` 子项一致，可以指定填充和描边样式。

常驻小画布同时需要 `island.pinned` 和 `status.canvas`，返回 `{"pinned":{"canvas":{"width":126,"nodes":[…]}}}`。高度固定为 22，宽度为 1–156，最多 60 个节点。可以使用绑定、数字翻动和横向滚动文字。横向滚动给 `text` 节点配置 `max_width` 与 `loop.kind:"flow"`，并放进带 `clip` 的分组。多种常驻内容可将 `canvas` 写成画布数组，随宿主每 8 秒的节奏轮换。[常驻画布样例](../../spike/examples/canvas-pinned-demo/plugin.json)

具有 `storage` 能力的插件可从「已安装」的「···」菜单导出或导入数据。备份 JSON 保存 `id`、`dataVersion` 与 `storage`。导入会用备份替换当前数据，并核对插件 ID、数据版本及 256 KiB 的存储限额。

坐标须为 -2000 至 2000 的有限数字，字号为 6 至 96，文字总计最多 2000 字。画布及其分组总计最多 1500 个图元，嵌套 8 层，路径命令合计最多 4000 个。画布或其中的图片引用不合规时，页面显示「插件画面有误」，开发者工具显示原因。宿主先画出完整首帧，后续绘制超过 4 毫秒预算时沿用缓存画面，并在开发者工具记录。

`canvas` 为对象时，展示结果上限为 64 KiB，私有存储另计。宿主默认使用岛仔字体，画布隐私遵循岛仔的「截图和录屏中隐藏」设置。

##### 实时绑定

把实时值写在画布的 `binds` 中。岛仔计算值并按最近的可见变化安排重绘，脚本保持原有刷新周期。

| 来源 | 声明与结果 |
| --- | --- |
| 计数 | `{counter:{start_value:0,per_second:0.02,start_at:1795400000,decimals:2}}`。同时提供数字和格式化文字，小数位为 0 至 4。开始前保持起始值，可用 `until` 指定停止时间 |
| 倒计时 | `{countdown:{target_at:1795431600,format:"HH:mm:ss"}}`。数字为剩余秒数，也支持文字格式 `d天 HH:mm`。到点数字归零，文字显示「已结束」 |
| 进度 | `{progress:[1795392000,1795424400]}`。在开始到结束之间从 0 到 1，区间外保持端点值 |
| 指针 | `{clock_angle:{zone:"America/New_York",hand:"hour"}}`。分钟粒度，`hand` 选择 hour 或 minute，12 点为 0 度，顺时针增加 |
| 现在 | `{now:"HH:mm",zone:"Asia/Shanghai"}`。提供时间文字，也支持 `HH:mm:ss`。省略 `zone` 时使用本机时区 |
| 日夜 | `{daylight:{zone:"Asia/Shanghai",hours:[6,18]}}`。当地时间前闭后开，区间内为 1，区间外为 0。结束小时小于开始小时表示隔天结束 |

时间戳用 Unix 秒，时区用 IANA 名称。文字用 `{名字}` 插入格式化值，原样保留未匹配的花括号内容。数字位置可用 `{bind:"work"}` 读取数字，也可用 `{bind:"work",map:[0,210]}` 将 0 至 1 映射到给定区间。颜色可用 `{bind:"day",pick:["#2E64B5","#FFC43D"]}`，数值达 0.5 时使用第二项。比如在 `rect` 的宽度绑定进度时，岛仔可以按显示像素精度更新，而 `now:"HH:mm"` 到下一分钟才更新。

##### 动画与操作

- 文字节点添加 `roll:true` 后，变化的数字在 220 毫秒内向上翻动。给节点添加稳定的 `id`，宿主可跨输出匹配同一节点。
- `animate:{ms:300,ease:"spring"}` 可让位置、尺寸、角度、透明度和颜色平滑过渡，新增或移除节点会淡入淡出。时长 120 至 1200 毫秒，缓动支持 linear 和 spring。
- `loop:{kind:"pulse",period_ms:1600}` 支持 pulse、flow、bob、spin。周期至少 800 毫秒，一个画布最多 8 个循环节点，循环最多 30 帧每秒。带 `id` 的节点可用 `burst:"confetti"` 播放一次彩纸。
- `{"pet":{"at":[226,24],"size":28,"mood":"expressions","motion":"run","id":"runner"}}` 由宿主绘制并沿用用户的表情包。尺寸为 12 至 96，`at` 是角色中心并支持绑定。表情可选轮换（`expressions`）、睡觉（`sleeping`）、开心（`happy`）或现有表情名。动作为原地跑跳（`run`）、浮动（`bob`）或静止（`none`）。每个展开画布最多一只，活动时计入循环预算。
- `hit:{action:"select"}`、`hit:{open:"https://example.com"}`、`hit:{copy:"复制这段"}` 三选一，复用标准组件交互规则。需要声明 `action` 能力，链接须在清单的 `network.hosts` 内，复制最多 2000 字。每画布最多 64 个点按区域，输出更新后旧点击失效。
- 节点可用 `hover:{opacity:1,lift:2,fill:"$accent",hint:"复制"}` 调节悬停外观，并显示最多 40 字的单行提示。提示限制在画布可见区域内。折线可用 `tips:[{x:140,label:"12:40",value:"4,106.2"}]` 声明宿主的悬停读数。
- 带 `clip` 的分组可加 `scroll:{axis:"y",content:420}`。滚轮在区域内移动内容，插件刷新会保留滚动位置。

[画布图元样例](../../spike/examples/canvas-demo/plugin.json)展示图元、渐变、包内图片，以及画布与标准组件混排的方式。脚本的 `canvas` 字段格式见[清单格式检查表的 `$defs.canvas`](sdk-v1.schema.json)。

## 6. 数据能留多久

用户可以停用插件一段时间再回来。宿主根据插件 ID 分开保存设置和私有数据，每次成功调用只替换本插件的数据。保留数据卸载后，从原清单位置重装开发包，或从原测试集市目录重装同 ID 试用包，可以找回原值。

本地版把安装快照放在配置目录的 local-plugins。正式版拟把只读程序放在 `%APPDATA%\Daozai\plugins\<ID>\<版本>`，用户内容放在同级 plugin-data 的对应 ID 目录。升级时在副本上按 dataVersion 转换旧数据，成功才切换，失败继续用旧包和旧数据。在模块设置页导出的文件包含用户内容和数据版本，密钥和文件授权留在本机。卸载撤销授权、删除凭据，选择保留的用户内容由通过身份核对的同一插件认领。

## 7. 插件多久刷新一次

作者在清单填写间隔，宿主负责安排工作。当前定时卡支持 60 至 86400 秒，旧样例缺省为 60 秒。首次启用或修改设置会重新计算。界面读取最近结果，每个插件同时最多一次调用，全局最多两个后台计算。停用时画面立即撤下，稍后返回的结果作废。

QuickJS 每次计算限时 30 毫秒，脚本可分配的内存上限为 4 MiB。单次展示输出最多 16 KiB，包含画布时为 64 KiB，私有存储另计，最多 256 KiB。普通刷新周期至少 60 秒，实时前台内容按需开放 1 秒。HTTPS 每插件最多 3 个来源，每次请求最长 10 秒，响应最多 256 KiB。拟保留 3 个固定插件卡位置和 1 个临时位置，满时让用户选择替换。

连续三次脚本执行失败后暂停并提示重试，是待实现的正式版规则。监控程序没启动时应提示打开依赖，设备离线时显示上次更新时间并降低查询频率。两种情况均属于数据来源状态，由宿主另行处理。

## 8. 作者交付哪些文件

从[示例目录](../../spike/examples/payday)开始，复制清单、入口和封面。清单中的 `entry` 指向包内入口。ID 使用小写作者名与插件名，以点分隔，最长 64 个字符。

version 是插件版本，api 是需要的接口主版本，dataVersion 是存储格式版本。`api` 高于宿主支持的版本时，安装会提示「需要更新岛仔」。owner 决定设置归属，capabilities 列出使用的能力，permissions 列出授权事项。字段类型和可选值见[清单格式检查表](sdk-v1.schema.json)。该表也描述设计中的能力，本地试用目前接受的组合见[本地安装说明](local-install.md)。

每个插件都必须提交一张封面。清单必须声明 `"cover": "cover.png"`，图片放在包根目录，使用有效 PNG，最大 256 KiB，最大 1920×1080。界面以 16:9 居中裁切，建议作者直接使用 16:9 并将主体放在画面中间。缺失或无效的封面会阻止试装、打包、提交和更新。旧版已装插件继续运行，原来缺图的条目显示占位，升级后显示新包的封面。

### 脚本设置与输入（实验）

`switch` 保存为 `"true"` 或 `"false"`。`choice` 声明 2 至 6 个 `{value,label}`，保存所选 value。`number` 声明 min、max、step，可加 unit、precision。`time` 使用 24 小时制 `HH:MM`。`weekdays` 使用逗号分隔的 1 至 7，从星期一开始。空字符串表示未选择星期。

`text` 可声明 `multiline: true`。输入框随内容长高，最多显示 6 行，超出可滚动。Enter 换行，Ctrl+Enter 保存设置。

`timezone` 设置用中文城市名、国家或地区名、IANA 时区代码搜索，例如「纽约」「美国」「America/New_York」。保存 IANA 时区名，例如 `America/New_York`。城市与地区数据沿用 Unicode CLDR 的中文名称和 IANA 时区数据库。

`list` 声明 1 至 7 个子设置 `item`，用 `maxItems` 指定最多 20 行。子设置支持 text、choice、number、date、time、switch、timezone、icon、color。保存为 JSON 数组字符串，每行对象的值仍为字符串，例如 `[{"city":"北京","zone":"Asia/Shanghai"}]`。脚本可用 `JSON.parse(settings.cities)` 读取。

`icon` 与 `color` 用于列表子项，保存名字字符串。`icon` 可选 `heart`（爱心）、`cake`（蛋糕）、`ring`（戒指）、`plane`（飞机）、`star`（星星）、`gift`（礼物）、`flag`（旗子）、`calendar`（日历）、`coffee`（咖啡）、`coin`（硬币）、`sun`（太阳）、`moon`（月亮）。

| `color` 值 | 对应颜色 |
| --- | --- |
| red | `#E26663` |
| orange | `#EF9A4F` |
| yellow | `#EBC253` |
| green | `#79AD67` |
| teal | `#4DADA5` |
| blue | `#6398E5` |
| purple | `#A07CD4` |
| pink | `#D880A6` |

`input.now` 是 Unix 毫秒，`input.tz` 提供本机 `{offset_minutes,name}`。清单声明 `timezones:true` 后，`input.zones` 包含设置中出现的有效 IANA 时区，值为 `{offset_minutes,dst}`。SDK 复用项目原有的 `chrono-tz` 依赖，在本机换算偏移和夏令时。

清单 `data` 可列出包根目录的 UTF-8 JSON，单个 64 KiB、合计 256 KiB。脚本通过 `input.data[文件名]` 读取。安装、测试集市和 `--plugin-pack` 都包含已声明的数据。节假日示例自带 2026 年数据，2027 年起由作者随新版本按年更新。

带 `timer` 的插件可返回 Unix 秒 `next_refresh_at`。下一次刷新取正常周期和指定时刻里较早的时间，至少间隔 60 秒。`storage` 单独限 256 KiB，其余输出限 16 KiB，包含画布时为 64 KiB。写入超限时保留原有数据，在插件详情提示错误。

宿主检查字段、ID、默认值、版本、入口和封面，并拒绝越出包目录的文件。重启时重新检查安装快照，出现无效记录提示重新安装。测试集市 ZIP 包含包根的 `plugin.json`、`cover.png`、清单指定的入口和数据文件。声明 `status.canvas` 时，`--plugin-pack` 也会打包符合要求的 PNG 和 SVG，保留包内相对路径。普通 ZIP 上限为 832 KiB，画布版 ZIP 上限为 1376 KiB，画布图片仍须遵守 512 KiB 合计上限。宿主核对目录中的大小与 SHA-256，直接在内存中读取包内容。正式资源包和独立模块的字段见清单格式检查表。

材质包包含 `plugin.json`、`cover.png`、一个大小不超过 64 KiB 的 `.wgsl` 入口，以及可选的文本说明文件。默认值必须是范围内的有限数字，步长为正且不大于范围宽度。`unit` 最多 4 个字符，仅显示在滑块数值后。`precision` 为 0 至 3 的整数，省略时按 `step` 的小数位数显示。显示格式不改变参数值。控件弹簧刚度范围为 0.2 至 3，阻尼范围为 0.1 至 1。

## 9. 岛仔升级后还能用吗

建议先稳定插件身份、基本设置、卡片、存储的含义。新增可选字段要给旧包保留默认值。更改单位或删除已承诺的字段，就提升接口主版本，让新旧规则分别处理对应的包。

独立模块、来源、实时歌词和事件先标实验。遇到不兼容的实验能力时，岛仔应列出受影响的部分。更新申请更多权限时先保留可用旧版，用户同意后切换。六个月的新旧主版本并行期建议只适用于已定稿的稳定接口，期限待确认。发版前用旧版官方插件的原包，在新宿主实测安装、数据恢复、旧设置、主题和启停。遇到不支持的版本时保留数据并提示升级。

## 10. 作者如何试装和检查

当前「开发者工具」支持选择本地清单安装。两个样例可分别启用，在所属设置页改日期，重启检查，再卸载并选择保留或删除数据。作者改脚本后可从原位置重新安装。[可执行的试用步骤](local-install.md)

开发包与集市成品分别识别。Windows 上启用 `sdk-js` 的试用版提供 `--plugin-test`、`--plugin-pack` 和 `--plugin-submit`。使用相同的清单规则，并用默认参数试跑一次插件。校验失败给出错误并返回非零退出码。作者可先在「开发者工具」试装，在改动后从原清单位置重新安装。自动重载和不同外观的预览是后续工作。[测试集市试用步骤](local-install.md#测试集市)

## 11. 怎样把原有代码拆出来

### 本地开发调试（实验）

在「设置 → 插件 → 开发者工具」打开「监视文件变化」。保存本地安装的插件清单、入口或包内数据文件后，岛仔会自动重载并保留私有数据。脚本可调用 `console.log(...values)`，每次最多 20 行、每行 200 字。开发者工具保留最近 100 行，日志只在本次运行期间可见。

运行 `daozai.exe --plugin-test plugin.json` 可试跑 JavaScript 插件的 `refresh`，查看返回值、运行耗时、报错位置及能力自检。追加 `--input mock.json` 传入可选的 `settings`、`storage`、`now`（Unix 毫秒）和 `sources`。只读 `input.source` 的插件可用 `{"sources":{"default":{"status":"ready","text":"模拟响应"}}}` 试跑。

先保留业务算法和测试，再把输入、展示结果和存储写成函数。喝水提醒原有设置限制为 5 至 180 分钟，可在清单的数字设置中声明范围，函数读取用户输入并返回展示内容。

```js
function refresh({settings}) {
    const minutes = Math.min(180, Math.max(5, settings.minutes));
    return {card: "提醒间隔 " + minutes + " 分钟", storage: {minutes}};
}
```

旧额度草案已标记废弃。迁移时从旧命令中提取余额计算，把接入端输出接到用户授权的本地文件或回环接口，把计算改成 refresh 并返回额度数据。旧凭据由接入端继续管理。[旧草案说明](README.md)

可直接交给 AI 的提示词：

> 阅读我的 PR、使用场景和岛仔 SDK。列出可复用的业务逻辑和已有测试。选择一个设置归属，把定时、按钮和事件写进清单。用 refresh、action 或 event 接收输入并返回结果。生成最小包和模拟数据，标清哪些能在当前宿主试装、哪些需要新接口。验证启停、重启和卸载恢复。保留原作者与素材许可。

#### 状态页交互、专注事件和提醒

在清单中同时声明 `action` 和 `status.page`。`status.actions` 最多 3 个按钮，每个按钮填写 `{id, label}`，标签 1 至 6 字。`status.fields` 最多 3 个输入框，每个填写 `{key, label, value?, max, multiline?}`，标签 1 至 28 字，`max` 为 1 至 200 的整数，`value` 是可选初始值。每组的 ID 或键须唯一，由小写英文字母开头，最多 40 个字符，可用小写英文字母、数字、下划线和连字符。自定义布局用 `actions`、`fields` 选择控件位置，默认布局将控件放在读数下方。

状态页字段默认单行。`multiline: true` 允许 Enter 换行，最多显示 6 行，超出可滚动。Ctrl+Enter 触发第一个交由脚本处理的 `status.actions` 按钮，表单会保留换行。

列表最多 8 项，每项可写 `{label, detail?, state?, action?, open?, copy?}`。`action` 是脚本动作 ID。按钮与列表项都可加 `open` 或 `copy`，二者只能选一个。`open` 打开清单 `network.hosts` 声明的 HTTPS 域名下的链接。`copy` 将最多 2000 字写入剪贴板。当行既有脚本动作又有复制内容时，点正文由脚本处理，点行尾「复制」由宿主处理。

点击脚本动作时，岛仔在后台调用 `action({settings, storage, today, source?, action:{id, form}})`。`form` 是字段键到文字的映射，未填的字段为 `""`。按钮和列表项只在生成它们的状态结果仍有效时接受点击。状态刷新、停用插件、修改设置或关闭所属模块都会使对应的旧结果失效。

清单声明 `event` 并列出 `"events":["focus.finished"]`。每次专注计时结束只投递一次。同一个插件已有调用时，事件按顺序等候。插件停用或所属模块关闭期间的事件不补发。全局函数 `event(input)` 收到与 `refresh` 相同的通用字段及 `event:{name, id, at, minutes}`，`at` 为 Unix 秒，`minutes` 为本轮配置的专注分钟数。

声明 `notice` 后，三个脚本入口都可返回 `notice:{id, title, body?}`。ID 为 1 至 128 字，标题为 1 至 40 字，正文可省略，最多 120 字。每个插件同一 ID 只显示一次，60 秒内最多发出一条。提醒显示和正文遵循岛仔现有的通知开关。

三个脚本入口共用每插件单次运行的限制，以及 30 ms、4 MiB 和 16 KiB 的展示输出上限，包含画布时展示上限为 64 KiB。可在本地试装[便签样例](../../spike/examples/sticky-notes/plugin.json)。

## 12. 投稿怎样进入集市

### 测试集市

当前用本机目录试用作者打包、提交，和用户安装、更新。作者校验开发包、生成 ZIP，再提交到 `catalog.json` 所在目录。每个插件 ID 在目录索引中列一个最新版本。已经提交的版本 ZIP 保持原文件，发布新版要递增三段式版本号。

索引为 UTF-8 JSON，顶层 `format: 1`、`plugins` 数组。每项包含 `id`、`name`、`author`、`version`、`size`（字节数）和 `sha256`（小写十六进制）。ZIP 文件名为 `plugin-<id>-<version>.zip`，和索引放在同一目录。作者命令负责填充元数据和校验值。

条目可选填 `revoked:["1.0.1"]` 标记撤回版本。宿主同步目录时会停用已安装的撤回版本，在「已安装」提示撤回，并阻止从该目录安装版本。有更高的可用版本时可更新，更新后可重新启用。

用户在「设置 → 插件 → 已安装 → 开发者工具」导入目录，集市切到「本地测试」。安装后在「已安装」管理。升级从同一目录领取同 ID、版本更高的包。更新验证通过后才替换安装快照，保留原启用选择、已保存的设置和私有数据。新增字段使用新版默认值。校验、试跑或兼容检查失败时保留旧包。

本地测试集市接受合规的 QuickJS 插件、外观资源包、快捷切换插件、无线设备兼容包以及 GPU 材质。申请本地数据源的 JS 插件仍需用户安装后单独授权。索引及 ZIP 应来自用户信任的本机目录。SHA-256 核对包是否与索引一致。

官方无线设备兼容包通过插件集市分发，在「已安装」管理。ZIP 包含 `plugin.json`、`wireless-receivers.json` 和 `cover.png`。接收器表补充已核实的 2.4G USB 接收器型号。

接收器表的 `api_version: 1` 接受 `vid`、`pid` 和 `name`。`api_version: 2` 的每条记录还可声明 `battery_protocol`，取值为[本体已注册的电量读取器](../../src/bt/compat.rs)。支持版本 2 的宿主遇到尚未内置的协议时仍识别接收器，电量显示为未知。旧格式仍可安装。插件清单的 `api` 和 `dataVersion` 均为 `1`。

「设备兼容 → 电量增强」可单独安装 HaloBattery 采集组件。开启后由岛仔校验并启动，关闭后停止采集。兼容包继续独立分发接收器识别表。

索尼 INZONE H9 使用接收器的 COM 接口。大疆 Mic Mini 2 需要接收器的 USB 接口 6 已绑定 WinUSB。采集组件按 Windows USB 物理实例绑定两个接口，和 HID 使用同一套岛仔通信格式。

采集组件从 [vendor/halo-battery/build.ps1](../../vendor/halo-battery/build.ps1) 构建，产物为 Windows x64 的 `daozai-halo.exe`。在 Gitee 插件镜像仓库创建 `wireless-battery-vX.Y.Z` 发行版并上传同名附件，验证匿名下载的文件一致后更新 `master` 上的 `wireless-battery/component.json`，写入 `version`、`size`（字节数）和 `sha256`。岛仔按这份清单校验组件后安装。桌面测试版可从程序旁的 `wireless-battery-test.json` 加载本地验收源，其中包含组件绝对路径 `artifact` 及 `package.size` 和 `package.hash`。

### 正式分发

作者向独立插件仓库提交 PR，提供源码、清单、封面、版本、变更、测试方法、许可和镜像分发授权。维护者审核并合并后打包。发布先把不可覆盖的 ZIP 上传到 Gitee，确认匿名下载的文件一致，再更新同目录的 `catalog.json`。[插件投稿仓](https://github.com/Renly1994/xiaotang-daozai-plugins)

用户打开「设置 → 插件 → 集市」时，岛仔优先显示本机保存的目录和统计，缺少缓存时自动加载一次。切换标签和关闭详情直接使用当前数据。点击「刷新目录」可重新检查 Gitee 目录和统计。断网或刷新失败时继续显示上次有效数据。安装前核对文件大小和 SHA-256，升级验证通过后才换包，保留已有数据和启用选择。验收版读取 `preview`，发布版读取 `stable`。[Gitee 下载镜像](https://gitee.com/xiaotangtoken/xiaotang-daozai-plugins)

发布者签名仍待实现。

### 详情、收藏与统计

集市索引可以为每项附上 `presentation`，由集市维护者填写。插件简介用 `description`，更新日期用 `updated_on`（`YYYY-MM-DD`），维护者账号用 `maintainer`。官方发布填 `official`，验收样例填 `sample`。官方插件显示「官方」，旧版官方清单的作者「岛仔」显示为「小唐」。头像可用同目录的 `avatar-*.png`，在 `author_avatar` 中填写文件名。先发布头像文件，再更新索引。缺头像时显示作者名称首字。

用户可从集市或详情收藏插件，在集市的「收藏」筛选里查看和取消。本机收藏按插件 ID 保存，后续版本和插件启停沿用同一收藏。历史安装条目如果没有封面，管理页显示占位，启停、设置和卸载仍可用。

集市的下载量和收藏量取自统计服务，预览与正式目录分别计数。下载量从服务启用起统计通过校验的安装包下载，同一事件补报只计一次。收藏量按匿名安装的当前收藏状态汇总，取消后扣减，已有本机收藏随目录同步补报。上报成功后，集市直接使用返回的统计数据。断网时仍可管理本机收藏，待联网后补报。集市尚未收到统计时显示「—」。上架新插件或版本时须在统计服务登记对应频道。

### 插件反馈

从详情或「已安装」的「···」打开「插件反馈」。普通问题和建议会在独立插件仓打开 GitHub 草稿，预填插件 ID、插件版本和岛仔版本，用户补充信息后自行提交。安装或更新异常由岛仔维护者跟进。没有 GitHub 账号时可复制反馈信息转交。安全或隐私问题选择「复制并打开私密报告」，登录 GitHub 后粘贴插件信息，补充并提交给有权限的维护者。[GitHub 私密报告](https://github.com/Renly1994/xiaotang-daozai-plugins/security/advisories/new)

插件上架时，仓库维护者在 `.github/plugin-maintainers.json` 登记作者或接手维护的 GitHub 账号。普通问题单提交后，仓库按插件 ID 自动提及对应账号。作者修复时提高版本号并提交关联原问题的 PR。维护者审核、发布和同步镜像后，在原问题下告知可用版本。

## 13. 需要小唐决定的事

这三项决定的是正式对外承诺，当前开发包按本文的全新调用方式运行。

| 决定 | 可选方式与建议 |
| --- | --- |
| 函数如何运行 | 每次全新运行，或为每个插件长期保留环境。建议前者，恢复只依赖输入和已保存数据，首版文档与样例统一采用 |
| 怎样读取用户指定的本机软件状态 | 监控可导出文件供用户选择，也可提供固定的回环 GET 接口。拓竹等接入端需说明依赖和接口地址，端口更改后由用户重新授权 |
| 旧额度命令如何退出 | 直接废弃，或为已有用户限期提供迁移期。建议停止接受新命令包，已有作者迁往受控来源接口。兼容期长度由小唐决定 |

生产阶段还需确认稳定接口的支持期限、并发与空间上限、独立进程、卸载默认选择、换维护者时的数据认领及市场签名密钥管理。

### 后续工作

以下内容还需要实际开发和试装。

可编辑的独立页面用便签验证。后续试验临时卡到期撤回、日程跨来源去重、切歌时清旧歌词，以及重复通知只提醒一次。用资源换肤包、纪念日验证切换恢复与跨模块输出。评估独立低权限进程对故障隔离和启动的影响。

### 名词解释

读清单或查看错误提示时，可以在这里对照字段的含义。

- **宿主**指安装和运行插件的岛仔。**SDK** 是作者接入岛仔所用的约定、样例和工具。
- **纯函数**指只用输入算出结果的函数。两次调用之间要保存的内容放在 `storage`。
- **QuickJS** 是宿主内运行 JavaScript 的程序。**KiB / MiB** 是容量单位，1 KiB 等于 1024 字节，1 MiB 等于 1024 KiB。
- **清单**是作者提交的 plugin.json。**ID** 是插件的固定名称。
- **接口**是宿主和插件约定的输入输出。**主版本**用来区分不兼容的规则。
- **JSON** 是由文字、数值、列表等组成的数据格式。**Schema** 是机器可读的字段检查表。
- **PR** 是向仓库提交代码或插件的审核请求。**SHA-256** 用来检查文件内容是否改变。**签名**用来确认发布者身份。
