//! Wayland / XDG 桌面入口安装。
//!
//! GPUI 的 Linux 平台层在 X11 下通过 `_NET_WM_ICON` 把 `WindowOptions::icon`
//! 写到窗口上；但 **Wayland 不支持 `_NET_WM_ICON`**，合成器（GNOME / KDE）
//! 只靠窗口的 `app_id` 去匹配同名 `.desktop` 文件，再从该文件 `Icon=`
//! 指向的图标主题目录里解析窗口 / 任务栏图标。`WindowOptions::icon` 在
//! Wayland 下会被直接忽略。
//!
//! 因此要让 Wayland 下 app 图标正确显示，必须向 XDG 目录安装：
//! - `~/.local/share/applications/dev.whalenest.desktop`（含
//!   `StartupWMClass` 与 `Icon=`，二者均取 `app_id` 的 basename）；
//! - `~/.local/share/icons/hicolor/<size>/apps/dev.whalenest.desktop.png`。
//!
//! 模块幂等：每次启动调用一次，已存在则跳过，失败静默（不影响主流程）。

use std::path::PathBuf;

/// `app_id`（同时用作 .desktop 文件名与图标名）。
pub const APP_ID: &str = "dev.whalenest.desktop";

/// 安装 Wayland / XDG 桌面入口。幂等，失败返回 Err（由调用方静默处理）。
pub fn install() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        install_icon()?;
        install_desktop_file()?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn data_home() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg);
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/share")
}

/// 把图标 PNG 写入 hicolor 图标主题目录（app_id.png），供合成器解析。
#[cfg(target_os = "linux")]
fn install_icon() -> Result<(), String> {
    // 编译期嵌入的源图（与 load_window_icon 共用同一张）。
    let bytes = include_bytes!("../public/whalenest-mark.png");
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;

    // 高分辨率源（1254x1254）采样缩放到 512，向下采样让边角干净。
    let icon = img.resize(512, 512, image::imageops::FilterType::Lanczos3);
    let rgba = icon.to_rgba8();

    let dir = data_home().join("icons/hicolor/512x512/apps");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建图标目录失败: {e}"))?;
    let dest = dir.join(format!("{APP_ID}.png"));

    // 编码一次，内容一致则跳过，避免每次启动重写（也避免 XDG 缓存抖动）。
    let mut buf: Vec<u8> = Vec::new();
    {
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| format!("编码图标 PNG 失败: {e}"))?;
    }

    if let Ok(existing) = std::fs::read(&dest) {
        if existing == buf {
            return Ok(());
        }
    }

    std::fs::write(&dest, &buf).map_err(|e| format!("写入图标失败: {e}"))?;
    Ok(())
}

/// 写 `.desktop` 文件到 XDG applications 目录，`StartupWMClass` / `Icon=`
/// 均取 `APP_ID`，从而与 `WindowOptions::app_id` 匹配。
#[cfg(target_os = "linux")]
fn install_desktop_file() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.display();

    let dir = data_home().join("applications");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建应用目录失败: {e}"))?;
    let path = dir.join(format!("{APP_ID}.desktop"));

    let content = format!(
        "[Desktop Entry]
Type=Application
Name=WhaleNest
Comment=DeepSeek Harness desktop shell
Exec=\"{exe}\"
Icon={APP_ID}
StartupWMClass={APP_ID}
Terminal=false
Categories=Development;Utility;
"
    );

    // 内容一致则跳过。
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if existing == content {
            return Ok(());
        }
    }

    std::fs::write(&path, content).map_err(|e| format!("写入 .desktop 失败: {e}"))?;
    Ok(())
}
