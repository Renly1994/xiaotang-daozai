# 上游反馈：吉祥物整身角色接入插件生态（建议新增 `appearance.pet-body`）

> 提交渠道：功能建议 Issue（`.github/ISSUE_TEMPLATE/feature_request.yml`）。
> CONTRIBUTING 第 3 行写明「提交较大的功能前，先开 Issue 说明具体场景与预期行为」，
> 所以这份内容先走 Issue，不直接开 PR；等作者认可接口形态再提 PR。

## 需求类型

主题、皮肤、宠物表情等外观

## 使用场景

我在做岛仔的定制发行版，给吉祥物接了「整身角色」素材库：除了原版矢量形象，
还内置了三套全身图集（银灰猫、圆脸银虎斑、prepre），每套按 11 个状态
（idle / hover / music / focus / sleep / happy / satisfied / warning / wake / doze / thirsty）
分别给帧宽、列数、帧数和帧速，绘制时整只替换掉矢量身体，设置页用同素材做放大预览。

现在这套只能编进宿主：`src/pet_assets.rs` 负责读图集，`Pet::draw` / `Pet::draw_expanded`
负责按状态取帧。结果是——

- 别人想加一个角色，必须改 Rust 代码、提 PR、等发版；
- 这和插件系统「做好后所有人都能用，也不用等岛仔发版」的目标正好相反；
- 而现有 `appearance.pet` 帮不上忙，它换的是**表情层**，不是整身角色。

## 希望的效果

新增一个 assets 运行时能力，比如 `appearance.pet-body`（`owner: "appearance"`，与现有
`appearance.pet` 同样独占），清单声明每个状态的图集来源，宿主复用现有的动作触发时机
取帧、在给定矩形内缩放绘制：

```json
{
  "id": "example.silver-cat",
  "runtime": "assets",
  "owner": "appearance",
  "capabilities": ["appearance.pet-body"],
  "entry": "body.json"
}
```

`body.json` 大致按状态给出源文件与该状态的帧参数（帧宽、列数、帧数、帧速、是否循环），
和现有 `pet_assets` 的 `Source` / `Clip` 字段一致即可，宿主沿用现在的
`advance(state, dt, visible)` 时序。

如果作者更希望扩展现有 `appearance.pet`（在同一条目里区分「表情层」与「整身层」），
也完全可以接受；关键是让整身角色不必编进宿主。

## 补充信息

调研到的现状（0.6.70 源码）：

| 位置 | 现状 |
|---|---|
| `spike/capabilities.rs:60` | `Capability { name: "appearance.pet", runtime: "assets", role: Role::Exclusive { owner: "appearance" } }` |
| `docs/plugins/sdk-v1-design.md:46` | 「吉祥物表情 · 测试版 … 沿用原动作的触发时机与帧速」 |
| `spike/appearance_assets.rs:462` | `appearance_pet()` 返回 `pet_expressions::ExpressionSheets`，只覆盖表情层 |
| `src/pet.rs` | 矢量身体由 `draw_face` + 后续矢量绘制完成，与表情层是两条路径 |
| `spike/examples/` | 目前没有任何 `expressions.json` 示例，该能力还没有可参考的包 |

不建议把整身绘制塞进 `appearance.pet` 的现有语义：两者的替换粒度不同（整身 vs 叠加层），
混在一起会让「关掉表情插件」的预期变得含糊。

## 勾选

- [x] 我了解接入特定服务、个人定制类的需求会优先交给插件实现
- [x] 我愿意自己把它做成插件分享给大家

---

## 附：不建议直接开 PR 的原因

当前没有可提交的代码改动——要让整身角色可插拔，需要宿主先开放绘制接口（新增能力或扩展现有
能力），属于接口设计变更。等作者确认形态后，我可以按他的设计提交实现 PR，并附一套角色素材作示例。
