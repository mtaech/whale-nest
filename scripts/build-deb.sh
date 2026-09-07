#!/usr/bin/env bash
# 打包 WhaleNest 为 .deb，产物 dist/whalenest_<ver>_<arch>.deb。
# 依赖: cargo、dpkg-deb / dpkg-shlibdeps（dpkg-dev）、ImageMagick（图标缩放）。
set -euo pipefail

cd "$(dirname "$0")/.."

NAME=whalenest
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)
ARCH=$(dpkg --print-architecture)
APP_ID=dev.whalenest.desktop   # 与应用内 src/desktop_entry.rs 的 APP_ID 保持一致
PKG="${NAME}_${VERSION}_${ARCH}"

# 1. release 构建
cargo build --release --bin "$NAME"

# 解析真实 target 目录（用户可能通过 ~/.cargo/config.toml 的 build.target-dir 重定向）
TARGET_DIR=$(cargo metadata --no-deps --format-version 1 \
  | grep -o '"target_directory":"[^"]*"' | head -1 | cut -d'"' -f4)
BIN="$TARGET_DIR/release/$NAME"

# 2. 组装 deb 目录
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
root="$stage/$PKG"
mkdir -p "$root/DEBIAN" "$root/usr/bin" "$root/usr/share/applications" \
         "$root/usr/share/icons/hicolor/512x512/apps" "$root/usr/share/doc/$NAME" \
         "$stage/debian"

install -m755 "$BIN" "$root/usr/bin/$NAME"

# 图标: 512 一张，由桌面环境缩放
magick public/whalenest-mark.png -resize 512x512 "$root/usr/share/icons/hicolor/512x512/apps/${APP_ID}.png"
chmod 644 "$root/usr/share/icons/hicolor/512x512/apps/${APP_ID}.png"

# 桌面入口: 文件名必须与应用运行时生成的一致（APP_ID + .desktop），否则菜单里会出现两个 WhaleNest
cat > "$root/usr/share/applications/${APP_ID}.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=WhaleNest
Comment=DeepSeek Harness desktop shell
Exec=/usr/bin/$NAME
Icon=$APP_ID
StartupWMClass=$APP_ID
Terminal=false
Categories=Development;Utility;
EOF

install -m644 LICENSE "$root/usr/share/doc/$NAME/copyright"

# 3. 由二进制实际链接的库自动计算运行时依赖（dpkg-shlibdeps 需要 debian/control 上下文，
#    放在 stage 根（$PKG 目录之外），不会被打进 deb）
cat > "$stage/debian/control" <<EOF
Source: $NAME
Section: utils
Priority: optional
Maintainer: WhaleNest Developers <dev@whalenest>
Package: $NAME
Architecture: $ARCH
EOF
DEPS=$(cd "$stage" && dpkg-shlibdeps -O "$PKG/usr/bin/$NAME" | sed 's/^shlibs:Depends=//')
cat > "$root/DEBIAN/control" <<EOF
Package: $NAME
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Maintainer: WhaleNest Developers <dev@whalenest>
Depends: $DEPS
Description: WhaleNest - DeepSeek Harness desktop shell
 GPUI 桌面壳，管理 DeepSeek Harness (dsh) profile 与插件，
 后台常驻内核，一键在浏览器打开 dsh Web UI。
EOF

mkdir -p dist
dpkg-deb --build --root-owner-group "$root" "dist/$PKG.deb"
echo "→ dist/$PKG.deb"
