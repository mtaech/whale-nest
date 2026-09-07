//! 插件管理操作：后台调用 dsh plugin --profile <name> <args>，对齐 dsh 的
//! 插件语义（pnpm 前转发 + 自动对账 dsh.profile.bundles）。
//!
//! dsh 的插件命令本质是把参数转发给 profile 目录里的 pnpm：
//!   - add <pkg>      安装插件（装完 dsh 自动把声明 dsh.bundle 的包加入 bundles）
//!   - remove <pkg>   卸载插件（并从 bundles 移除）
//!   - update <pkg>   升级到最新
//!   - outdated       列出可升级的包（升级前用它做校验，避免无谓升级）
//!   - why <pkg> / list 等 pnpm 子命令
//!
//! 本模块只负责"起后台线程 + 捕获输出 + 广播 AppEvent 结果"，不做磁盘写操作，
//! 不绕过 dsh 的对账逻辑 —— 完全交由 dsh/pnpm 处理，从而与 dsh 行为一致。

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::app::{self, AppEvent, Managed};
use crate::kernel::KernelState;

/// 单次插件操作的超时（安装/升级可能较慢，给足时间）。
const OP_TIMEOUT: Duration = Duration::from_secs(300);

/// 在后台执行一次 dsh plugin --profile <profile> <args...> 操作。
///
/// - 立即广播 PluginOpStarted 让 UI 进入"进行中"态。
/// - 完成后广播 PluginOpResult（成功/失败 + 提示）。
/// - 若该 profile 正在运行，操作成功后自动重启内核使变更生效。
pub fn run(managed: &Managed, profile: String, args: Vec<String>, action: &str) {
    let managed = managed.clone();
    let start_message = format!("正在{action}…");
    let profile_for_event = profile.clone();
    let _ = managed
        .events
        .send(AppEvent::PluginOpStarted { profile: profile_for_event, message: start_message });

    std::thread::spawn(move || {
        let result = execute(&profile, &args);
        match result {
            Ok(output) => {
                let ok = true;
                let message = summarize(&args, &output, true);
                // 成功后对该 profile 重启内核（若在跑）使变更生效。
                let running = {
                    let k = managed.kernel.lock();
                    k.config.profile == profile && !matches!(k.state, KernelState::Stopped)
                };
                if running {
                    app::restart_kernel_impl(&managed);
                }
                let _ = managed.events.send(AppEvent::PluginOpResult {
                    profile: profile.clone(),
                    ok,
                    message,
                });
            }
            Err(e) => {
                let _ = managed.events.send(AppEvent::PluginOpResult {
                    profile: profile.clone(),
                    ok: false,
                    message: e,
                });
            }
        }
    });
}

/// 同步执行 dsh plugin --profile <profile> <args>，返回合并输出。
fn execute(profile: &str, args: &[String]) -> Result<String, String> {
    let mut cmd = Command::new("dsh");
    cmd.arg("plugin")
        .arg("--profile")
        .arg(profile)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("启动 dsh 失败: {e}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_handle = stdout.map(|mut s| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            buf
        })
    });
    let err_handle = stderr.map(|mut s| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            buf
        })
    });

    let deadline = std::time::Instant::now() + OP_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("插件操作超时（5 分钟）".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("等待 dsh 失败: {e}")),
        }
    };

    let out_text = out_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let err_text = err_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let combined = format!("{out_text}{err_text}").trim().to_string();

    if status.success() {
        Ok(combined)
    } else {
        Err(if combined.is_empty() {
            format!("dsh 插件操作失败（退出码 {:?}）", status.code())
        } else {
            combined
        })
    }
}

/// 从 pnpm outdated --format json 的输出提取 (包名, 最新版本) 列表。
/// 输出形如 {"pkg-name": {"current": "1.0", "latest": "2.0"}, ...}；空 {} 表示全部最新。
/// 解析失败或格式不符时返回空列表（视为无新版）。
fn parse_outdated_json(text: &str) -> Vec<(String, String)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Vec::new();
    };
    let Some(obj) = v.as_object() else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = obj
        .iter()
        .filter_map(|(name, meta)| {
            let latest = meta.get("latest").and_then(|x| x.as_str()).unwrap_or("");
            Some((name.clone(), latest.to_string()))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}


/// 查询当前 profile 可升级的插件 (包名, 最新版本)（走 dsh plugin ... outdated --format json）。
fn outdated_packages(profile: &str) -> Result<Vec<(String, String)>, String> {
    let args = vec!["outdated".to_string(), "--format".to_string(), "json".to_string()];
    let out = execute(profile, &args)?;
    Ok(parse_outdated_json(&out))
}

/// 升级前校验：目标包确有新版才升级，否则明确告知"已是最新"。
/// 复用 run 的广播模式，避免 UI 直接无脑 update。
pub fn update_checked(managed: &Managed, profile: String, package: String) {
    let managed = managed.clone();
    let start_message = format!("正在检查「{package}」是否有新版…");
    let profile_for_event = profile.clone();
    let _ = managed
        .events
        .send(AppEvent::PluginOpStarted { profile: profile_for_event, message: start_message });

    std::thread::spawn(move || {
        let outdated = match outdated_packages(&profile) {
            Ok(o) => o,
            Err(e) => {
                let _ = managed.events.send(AppEvent::PluginOpResult {
                    profile: profile.clone(),
                    ok: false,
                    message: format!("检查新版失败：{e}"),
                });
                return;
            }
        };

        let has_newer = outdated.iter().any(|(n, _)| n == &package);
        if !has_newer {
            let _ = managed.events.send(AppEvent::PluginOpResult {
                profile: profile.clone(),
                ok: true,
                message: format!("「{package}」已是最新版本，无需升级"),
            });
            return;
        }

        // 确有新版，执行实际升级。
        let upd_args = vec!["update".to_string(), package.clone()];
        let result = execute(&profile, &upd_args);
        match result {
            Ok(output) => {
                let message = summarize(&upd_args, &output, true);
                let running = {
                    let k = managed.kernel.lock();
                    k.config.profile == profile && !matches!(k.state, KernelState::Stopped)
                };
                if running {
                    app::restart_kernel_impl(&managed);
                }
                let _ = managed.events.send(AppEvent::PluginOpResult {
                    profile: profile.clone(),
                    ok: true,
                    message,
                });
            }
            Err(e) => {
                let _ = managed.events.send(AppEvent::PluginOpResult {
                    profile: profile.clone(),
                    ok: false,
                    message: e,
                });
            }
        }
    });
}

/// 根据操作类型给用户一个易读的结果提示。
fn summarize(args: &[String], output: &str, ok: bool) -> String {
    let first = args.first().map(String::as_str).unwrap_or("");
    let target = args.get(1).cloned().unwrap_or_default();
    let detail = if output.is_empty() { "完成".to_string() } else { output.chars().take(120).collect() };
    if !ok {
        return format!("插件操作失败：{detail}");
    }
    match first {
        "add" => format!("已安装插件「{target}」· {detail}"),
        "remove" | "rm" => format!("已卸载插件「{target}」· {detail}"),
        "update" | "up" => format!("已升级插件「{target}」· {detail}"),
        "outdated" => format!("检查完成：{detail}"),
        _ => format!("插件操作完成：{detail}"),
    }
}

/// 查询某 profile 的可升级插件并广播 PluginOutdated 事件（供 UI 显示"可升级"徽标）。
pub fn query_outdated(managed: &Managed, profile: String) {
    let managed = managed.clone();
    std::thread::spawn(move || {
        let updates = outdated_packages(&profile).unwrap_or_default();
        let _ = managed.events.send(AppEvent::PluginOutdated { profile, updates });
    });
}

/// 便捷入口：安装插件。
pub fn add(managed: &Managed, profile: String, package: String) {
    run(managed, profile, vec!["add".to_string(), package], "安装插件");
}

/// 便捷入口：卸载插件。
pub fn remove(managed: &Managed, profile: String, package: String) {
    run(managed, profile, vec!["remove".to_string(), package], "卸载插件");
}

/// 便捷入口：升级插件（带"确有新版"校验）。
pub fn update(managed: &Managed, profile: String, package: String) {
    update_checked(managed, profile, package);
}

#[cfg(test)]
mod tests {
    use super::parse_outdated_json;

    #[test]
    fn parses_empty_json_as_no_outdated() {
        assert!(parse_outdated_json("{}").is_empty());
    }

    #[test]
    fn parses_whitespace_as_no_outdated() {
        assert!(parse_outdated_json("   
  ").is_empty());
    }

    #[test]
    fn parses_outdated_packages_from_json() {
        let out = r#"{"dsh-ast-edit-tool":{"current":"0.1.1","latest":"0.2.0"},"dsh-better-sidebar":{"current":"0.18.0","latest":"0.19.0"}}"#;
        let got = parse_outdated_json(out);
        assert_eq!(
            got,
            vec![
                ("dsh-ast-edit-tool".to_string(), "0.2.0".to_string()),
                ("dsh-better-sidebar".to_string(), "0.19.0".to_string())
            ]
        );
    }

    #[test]
    fn malformed_json_is_empty() {
        assert!(parse_outdated_json("not json").is_empty());
    }

    #[test]
    fn non_object_json_is_empty() {
        assert!(parse_outdated_json("[1,2,3]").is_empty());
    }

    #[test]
    fn parses_latest_versions_for_every_package() {
        let out = r#"{"a":{"current":"1","latest":"2"},"b":{"current":"1","latest":"1"}}"#;
        let got = parse_outdated_json(out);
        assert_eq!(
            got,
            vec![
                ("a".to_string(), "2".to_string()),
                ("b".to_string(), "1".to_string())
            ]
        );
    }
}
