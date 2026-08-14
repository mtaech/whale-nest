---
name: WhaleNest
description: 晨光灯塔——浅色、克制、精致的 dsh 桌面壳视觉系统
colors:
  morning-sky: "#f6f8fb"
  signal-blue: "#2563eb"
  signal-blue-hover: "#3b76f5"
  ink-deep: "#1c2536"
  mist-gray: "#59687d"
  fog-gray: "#8792a6"
  warning-coral: "#d84343"
  harbor-line: "#dbe1ea"
  harbor-strong: "#c8d1df"
  tide-code: "#eef1f6"
  tide-ink: "#33415a"
typography:
  title:
    fontFamily: "system-ui, -apple-system, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif"
    fontSize: "1.12rem"
    fontWeight: 600
    lineHeight: 1.45
    letterSpacing: "-0.015em"
  body:
    fontFamily: "system-ui, -apple-system, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif"
    fontSize: "0.9rem"
    fontWeight: 400
    lineHeight: 1.8
  label:
    fontFamily: "system-ui, -apple-system, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif"
    fontSize: "0.83rem"
    fontWeight: 400
    lineHeight: 1.7
  mono:
    fontFamily: "'Cascadia Code', Consolas, 'SF Mono', Menlo, monospace"
    fontSize: "0.84rem"
    fontWeight: 400
rounded:
  sm: "7px"
  md: "9px"
  lg: "10px"
spacing:
  sm: "8px"
  md: "24px"
  lg: "40px"
components:
  button-primary:
    backgroundColor: "{colors.signal-blue}"
    textColor: "#ffffff"
    rounded: "{rounded.md}"
    padding: "9px 24px"
  button-primary-hover:
    backgroundColor: "{colors.signal-blue-hover}"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.mist-gray}"
    rounded: "{rounded.sm}"
    padding: "4px 12px"
---

# Design System: WhaleNest

## Overview

**Creative North Star: "晨光灯塔"**

WhaleNest 的界面是清晨海面上的灯塔：背景是海雾散后的明亮天空（`#f6f8fb`），唯一的色彩仍是天边那一点信号蓝光（`#2563eb`）——克制、坚定、只出现在需要引航的地方。整个系统服务于一个工具性前提：壳页只在 dsh 内核未就绪/出错/缺失时短暂出现，它不该与 dsh 的实际内容争夺注意力，而应以安静的亮色精致感完成「等待」这件事本身。

布局密度低、留白充分，垂直居中；视觉层级靠字重与间距而非装饰。动效只保留一个有意义的时刻（视图进入、波浪呼吸、进度滑动），全部遵循 `prefers-reduced-motion`。亮色下尤其克制——浅底意味着对比要靠字重与色阶，而非堆叠的边框或投影。

**Key Characteristics:**
- 浅色优先，单一信号蓝点缀（accent 使用面积 ≤ 10%）
- 无硬阴影、无玻璃拟态、无渐变文字——深度靠色阶与细边框
- 细线（2.5px）SVG 图标替代 emoji，几何而非写实
- 动效克制：一个视图一个作者时刻，其余静止
- 圆角温和（7-10px），按钮有轻微按压反馈

## Colors

以清晨海天为底、灯塔光为魂的单色点缀系统。

### Primary
- **信号蓝** (`#2563eb`): 唯一的行为色。用于主按钮背景、进度条、焦点环、选中态。它代表"行动/就绪"；使用面积必须克制（≤10% 屏幕），稀有即意义。
- **信号蓝·悬停** (`#3b76f5`): 主按钮 hover。同色相提亮，保持灯塔光的连续感。

### Neutral
- **晨空** (`#f6f8fb`): 全局背景。极浅的冷白，顶部叠加一层微弱的信号蓝环境光（radial-gradient，6% 透明度）。清澈、安静，不刺眼。
- **墨青** (`#1c2536`): 主文字。深蓝黑的墨色，用于标题与正文（对比度 ≥ 12:1）。
- **雾灰** (`#59687d`): 次级文字。用于副标题、说明、引导文案（对比度 ≥ 4.5:1）。
- **淡雾灰** (`#8792a6`): 最低调文字。仅用于版本号等装饰性小字。
- **海港线** (`#dbe1ea`): 边框/分隔。输入框、代码块、ghost 按钮的描边。
- **海港粗线** (`#c8d1df`): hover/强化描边。边框在交互时加深，替代阴影。
- **潮汐底** (`#eef1f6`): 代码块背景。比背景深一度的凹陷面。
- **潮汐墨** (`#33415a`): 代码块的等宽文字。

### Tertiary
- **警示珊瑚红** (`#d84343`): 仅错误态。错误图标、错误信息文字与边框。红色在系统中只表达失败，不参与其他语义（浅底上加深至可读）。

### Named Rules
**The 灯塔光 Rule.** 信号蓝在任意屏幕上的使用面积不超过 10%。它的稀有性是整个系统的克制宣言——当蓝色出现，它必然意味着"可以行动"或"已经就绪"。

## Typography

**Display Font:** system-ui 栈（Segoe UI / PingFang SC / Microsoft YaHei）
**Body Font:** system-ui 栈
**Label/Mono Font:** Cascadia Code / Consolas（代码与命令）

**Character:** 系统字体栈，不引入 web font——壳页是工具，字体就该安静地工作。层级靠字重（400/600）与字号（0.68-1.12rem）区分，而非花哨的字体配对。

### Hierarchy
- **Title** (600, 1.12rem, 1.45): 视图主标题（"正在启动…"、"内核启动失败"）。letter-spacing -0.015em，收紧凑。
- **Body** (400, 0.9rem, 1.8): 说明文字与错误信息。最大行宽 40ch。
- **Label** (400, 0.83rem, 1.7): 副标题/次要说明。letter-spacing 0.01em。
- **Mono** (400, 0.84rem, 1.4): 安装命令、代码。`user-select: text` 允许复制。
- **Version** (400, 0.68rem, 字距 0.22em, 大写): 版本号小字。宽字距 + 大写制造"刻印"感。

### Named Rules
**The 安静字体 Rule.** 不引入展示字体。工具的字体应该隐身——字号、字重、字距是唯一的分层工具。

## Layout

单一居中布局：`.shell` 最大宽 480px，垂直水平居中，内边距 48px 上下、32px 左右。所有视图（loading / error / guide）共享同一骨架：图标区 → 标题 → 说明 →（操作按钮）。

间距节奏：图标与标题之间 24px，标题与副标题 8px，区块之间 24-28px，品牌区到标题 40px。留白是层级的主要工具——区块靠间距分离，不靠边框卡片。

响应式：窗口最小 900×600，壳页始终可完整容纳；无多栏，无断点变换。

## Elevation & Depth

**无阴影系统。** 壳页是扁平界面，深度完全靠色阶传达：背景（晨空）→ 凹陷面（潮汐底，代码块）→ 悬浮元素（按钮用纯色填充，不投影）。

唯一的环境光来自背景顶部的 radial-gradient 信号蓝光晕（6% 透明度）——它不是阴影，是"清晨海面"本身的质感。按钮按压用 `transform: scale(0.98)` 表达，而非阴影位移。浅底上，交互常靠描边加深（border-color）而非投影。

**The 扁平默认 Rule.** 表面静止时全部扁平。没有投影、没有悬浮卡片；深度只由色阶提供。

## Shapes

圆角语言温和而克制：主按钮 9px、ghost 按钮 7px、代码块 10px、错误信息 10px。没有胶囊形（pill）大按钮——圆角只服务于"不扎手"，不表达"圆润可爱"。

图标统一 2.5px 描边、圆头端点（stroke-linecap: round）、几何构图（圆/弧/线段），viewBox 48×48。波浪 logo 为 2.5px 三条开放曲线，不是写实鲸鱼。

边框一律 1px 实线（海港线色），用于凹陷表面与弱操作；主按钮无边框（纯色填充）。

## Components

### Buttons
- **Shape:** 温和圆角（primary 9px，ghost 7px），1px 边框或无边框
- **Primary（信号蓝）:** `background: #2563eb; color: #fff; padding: 9px 24px`。hover 提亮至 `#3b76f5`；`:active` 微缩 `scale(0.98)`
- **Ghost（弱操作）:** 透明底 + 海港线 1px 边框 + 雾灰文字（7px 圆角，4px 12px 内边距）。hover 边框加深至海港粗线、文字变墨青
- **Focus:** `:focus-visible` 信号蓝 2px 焦点环 + 2px offset，键盘可达

### Status Icons
- **Style:** 2.5px 描边 SVG，圆头端点。错误态（警示珊瑚红）：细线圆 + 竖线 + 圆点；引导态（雾灰）：终端提示符 > 形
- **Size:** 错误 48×48，引导 52×52，居中于 56px 容器

### Code Block
- **Style:** 潮汐底背景 + 海港线 1px 边框 + 10px 圆角，等宽字体潮汐墨 `#33415a`
- **Text:** 命令文字 `user-select: text` 可复制；内部 ghost 按钮"复制"

### Progress Bar
- **Style:** 2px 高、168px 宽、胶囊底（墨青 10% 透明），38% 宽的信号蓝滑块以 1.3s cubic-bezier(0.45,0,0.55,1) 往复滑动
- **Role:** 无限不确定进度（indeterminate），表示"正在启动"而非定量进度

### Brand Mark（波浪 logo）
- **Style:** 三条 2.5px 开放曲线（back/mid/front），5.5s 错峰上下漂移（-2px）+ 透明度 0.55→1 呼吸
- **Role:** 加载视图的视觉锚点，呼应 WhaleNest「鲸巢/波浪」意象；`aria-hidden`

## Do's and Don'ts

### Do:
- **Do** 让信号蓝保持稀有——它只出现在"可行动/已就绪"的语义点
- **Do** 用间距分离区块，而不是给每个区块加边框卡片
- **Do** 用 2.5px 圆头细线 SVG 表达图标，几何构图
- **Do** 为键盘焦点提供信号蓝焦点环（2px + 2px offset）
- **Do** 遵守 `prefers-reduced-motion`——动效关闭后界面必须完整可用
- **Do** 保持标题字距 -0.015em、版本号字距 0.22em 的对比
- **Do** 让次级文字（雾灰）在浅底上维持 ≥ 4.5:1 的对比度

### Don't:
- **Don't** 使用 emoji 或 unicode 字符充当图标——必须用一致描边的 SVG
- **Don't** 使用渐变文字、玻璃拟态（backdrop-blur）、或零偏移装饰性光晕
- **Don't** 引入展示字体/web font——系统字体栈是承诺
- **Don't** 给表面加投影——深度只来自色阶
- **Don't** 在非错误语境使用警示珊瑚红
- **Don't** 用胶囊形大按钮或超过 16px 的圆角
- **Don't** 在浅底上使用低对比度文字（深灰对白是承诺，避免浅灰淡灰）
