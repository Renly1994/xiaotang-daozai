# 托盘图标登记恢复实施计划

> 依据：`docs/superpowers/specs/2026-11-19-tray-icon-recovery-design.md`

**目标：** 修复岛仔托盘图标未登记时无恢复能力的问题，补充可取证诊断，并在登记失败时自动有界重试。

**架构：** 保留原生 `Shell_NotifyIconW` 托盘结构，将登记动作抽成可注入的小边界，增加非阻塞重试定时器与登记状态诊断。

**技术栈：** Rust 2021、windows 0.58、eframe/egui 0.31、windows 消息循环定时器。

---

## 文件结构

- Modify：`src/tray.rs`：登记结果结构、可注入登记、重试定时器、诊断输出。
- Test：`src/tray.rs` 内 `mod tests`：登记成功/失败/回退/重试上限回归。
- Modify：`docs/superpowers/specs/2026-11-19-tray-icon-recovery-design.md`：补登记失败根因（完整性级别）记录。

---

### Task 1：登记结果与可注入边界

- [ ] Step 1：写失败测试：`NIM_ADD` 失败且 `NIM_MODIFY` 失败时返回失败阶段，不得误报成功。

```rust
#[test]
fn tray_registration_reports_failure_without_claiming_success() {
    let calls = std::cell::RefCell::new(Vec::new());
    let result = register(TrayIcon {hWnd: HWND::default(), uID: TRAY_UID}, |nid, op| {
        calls.borrow_mut().push((op, nid.uID));
        false
    });
    assert!(!result.registered);
    assert_eq!(calls.borrow().len(), 2, "ADD 失败后还要尝试 MODIFY");
}
```

- [ ] Step 2：运行测试确认失败。

```powershell
. .\target\build-tools\env.ps1
cargo +stable-x86_64-pc-windows-msvc test --target x86_64-pc-windows-msvc --lib tray::tests::tray_registration -- --nocapture
```

预期：编译失败（`register` 未定义）。

- [ ] Step 3：最小实现：新增 `Registration` 结果结构、`register` 函数，真实实现调用 `Shell_NotifyIconW`。

- [ ] Step 4：运行测试确认通过。

- [ ] Step 5：运行既有托盘测试，确认菜单、图标资源、Agent 消息不回归。

### Task 2：非阻塞有界重试

- [ ] Step 1：写失败测试：持续失败时重试次数不超过上限；成功后不再重试；`TaskbarCreated` 可重新开始恢复序列。

- [ ] Step 2：运行确认失败。

- [ ] Step 3：最小实现：托盘线程增加 `TIMER_RETRY`（如 13），首次失败时设置递增间隔定时器，成功或达上限时 `KillTimer`。

- [ ] Step 4：运行确认通过。

- [ ] Step 5：既有回归全部通过。

### Task 3：诊断与交付

- [ ] Step 1：`WM_TRAY_DUMP` 输出补充 `icon_registered`、`icon_attempts`、`icon_last_error`。
- [ ] Step 2：`git diff --check`。
- [ ] Step 3：完整 Rust 回归（`--all-targets --features record --target x86_64-pc-windows-msvc -- --test-threads=1`）。
- [ ] Step 4：release 构建并更新桌面固定入口。
- [ ] Step 5：实机核对托盘登记、菜单、快捷方式目标、SHA-256 与进程路径。
