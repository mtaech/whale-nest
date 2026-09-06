//! 桌面通知（尽力而为，失败静默）。
//!
//! - Linux：notify-rust（DBus org.freedesktop.Notifications）
//! - Windows / macOS：暂无实现（GPUI 无系统通知 API；保持编译可用）

/// 发一条系统通知。任何失败都被吞掉 —— 通知永远不应影响主流程。
pub fn notify(title: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        use notify_rust::Notification;
        let _ = Notification::new()
            .summary(title)
            .body(body)
            .appname("WhaleNest")
            .show();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (title, body);
    }
}