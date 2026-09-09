//! WhaleNest 视觉系统（晨光灯塔 · 浅色精致）
//!
//! 提供品牌主题注入、主题色阶常量与自定义高保真样式。

use gpui_kit::{App, Hsla, Rgba, Window, rgb};
use gpui_kit::component::theme::{Theme, ThemeMode, ThemeRegistry};

#[allow(dead_code)]
pub const COLOR_SIGNAL_BLUE: u32 = 0x2563eb;
#[allow(dead_code)]
pub const COLOR_SIGNAL_BLUE_HOVER: u32 = 0x3b82f6;
#[allow(dead_code)]
pub const COLOR_SIGNAL_BLUE_ACTIVE: u32 = 0x1d4ed8;

#[allow(dead_code)]
pub const COLOR_MORNING_SKY: u32 = 0xf8fafc;
#[allow(dead_code)]
pub const COLOR_INK_DEEP: u32 = 0x0f172a;
#[allow(dead_code)]
pub const COLOR_MIST_GRAY: u32 = 0x64748b;
#[allow(dead_code)]
pub const COLOR_HARBOR_LINE: u32 = 0xe2e8f0;

#[allow(dead_code)]
pub const COLOR_STATUS_RUNNING: u32 = 0x10b981;
#[allow(dead_code)]
pub const COLOR_STATUS_STARTING: u32 = 0xf59e0b;
#[allow(dead_code)]
pub const COLOR_STATUS_STOPPED: u32 = 0x64748b;
#[allow(dead_code)]
pub const COLOR_STATUS_ERROR: u32 = 0xef4444;

#[allow(dead_code)]
pub const COLOR_TERMINAL_BG: u32 = 0x0f172a;
#[allow(dead_code)]
pub const COLOR_TERMINAL_HEADER: u32 = 0x1e293b;

pub fn color_hsla(hex: u32) -> Hsla {
    let rgba: Rgba = rgb(hex);
    rgba.into()
}

pub fn color_hsla_opacity(hex: u32, alpha: f32) -> Hsla {
    let mut hsla = color_hsla(hex);
    hsla.a = alpha;
    hsla
}

/// WhaleNest 晨光灯塔主题 JSON
pub const WHALENEST_THEME_JSON: &str = r##"{
  "name": "WhaleNest",
  "author": "WhaleNest",
  "themes": [
    {
      "is_default": true,
      "name": "WhaleNest Light",
      "mode": "light",
      "radius": 8,
      "radius.lg": 12,
      "colors": {
        "background": "#f8fafc",
        "foreground": "#0f172a",
        "border": "#e2e8f0",
        "primary.background": "#2563eb",
        "primary.hover.background": "#3b82f6",
        "primary.active.background": "#1d4ed8",
        "primary.foreground": "#ffffff",
        "accent.background": "#eff6ff",
        "accent.foreground": "#1d4ed8",
        "muted.background": "#f1f5f9",
        "muted.foreground": "#64748b",
        "popover.background": "#ffffff",
        "popover.foreground": "#0f172a",
        "sidebar.background": "#f8fafc",
        "sidebar.border": "#e2e8f0",
        "sidebar.foreground": "#1e293b",
        "tab.background": "#00000000",
        "tab.active.background": "#ffffff",
        "tab.active.foreground": "#0f172a",
        "tab_bar.background": "#f1f5f9",
        "success.background": "#10b981",
        "success.foreground": "#ffffff",
        "warning.background": "#f59e0b",
        "warning.foreground": "#ffffff",
        "danger.background": "#ef4444",
        "danger.foreground": "#ffffff",
        "ring": "#3b82f6",
        "input.border": "#e2e8f0"
      }
    }
  ]
}"##;

/// 初始化并激活 WhaleNest 晨光灯塔主题。
pub fn init_theme(window: Option<&mut Window>, cx: &mut App) {
    if let Err(e) = ThemeRegistry::global_mut(cx).load_themes_from_str(WHALENEST_THEME_JSON) {
        eprintln!("[whalenest] 加载晨光灯塔主题失败: {e}");
        return;
    }

    if let Some(theme) = ThemeRegistry::global(cx).themes().get("WhaleNest Light").cloned() {
        Theme::global_mut(cx).light_theme = theme;
    }

    Theme::change(ThemeMode::Light, window, cx);
}

/// 动态切换全局字体。若 font 为 None 或为空，则回退为系统默认字体。
pub fn set_font_family(font: Option<&str>, window: Option<&mut Window>, cx: &mut App) {
    let font_family: gpui_kit::SharedString = match font {
        Some(f) if !f.is_empty() && f != "default" && f != "系统默认" && f != ".SystemUIFont" => {
            f.to_string().into()
        }
        _ => ".SystemUIFont".into(),
    };
    let mut config = (*Theme::global(cx).light_theme).clone();
    config.font_family = Some(font_family.clone());
    Theme::global_mut(cx).light_theme = std::rc::Rc::new(config);
    Theme::global_mut(cx).font_family = font_family;
    if let Some(window) = window {
        window.refresh();
    }
}
