#!/usr/bin/env bash
# 打一个可发布的压缩包：编译 release，连同 README 与 LICENSE 一起打包，
# 并输出 SHA256 校验值（可直接贴到 GitHub Release 说明里）。
#
# 用法（需要 bash；Windows 上用 git-bash）:
#     bash scripts/package.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
CRATE="$ROOT/rust/snap"
DIST="$ROOT/dist"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$CRATE/Cargo.toml" | head -1)"
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
NAME="snap-$VERSION-$TRIPLE"

echo "版本:      $VERSION"
echo "构建目标:  $TRIPLE"
echo

# --locked：完全按仓库里的 Cargo.lock 构建，保证产物可复现
( cd "$CRATE" && cargo build --release --locked )

case "$TRIPLE" in
    *windows*) BIN="snap.exe" ;;
    *)         BIN="snap" ;;
esac

STAGE="$DIST/$NAME"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "$CRATE/target/release/$BIN" "$STAGE/"
cp "$ROOT/README.md" "$ROOT/LICENSE" "$STAGE/"
if [ -f "$ROOT/CHANGELOG.md" ]; then cp "$ROOT/CHANGELOG.md" "$STAGE/"; fi
cp "$CRATE/README.md" "$STAGE/README-设计与性能.md"

# 打包（Windows 用 PowerShell 的 Compress-Archive，其它平台用 zip）
cd "$DIST"
rm -f "$NAME.zip" "$NAME.zip.sha256"
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        powershell -NoProfile -Command "Compress-Archive -Path '$NAME' -DestinationPath '$NAME.zip' -Force"
        HASH="$(powershell -NoProfile -Command "(Get-FileHash '$NAME.zip' -Algorithm SHA256).Hash.ToLower()" | tr -d '\r')"
        ;;
    *)
        zip -qr "$NAME.zip" "$NAME"
        HASH="$(sha256sum "$NAME.zip" | cut -d' ' -f1)"
        ;;
esac

printf '%s  %s\n' "$HASH" "$NAME.zip" > "$NAME.zip.sha256"

echo
echo "产物:"
ls -la "$NAME.zip"
echo
echo "SHA256:"
cat "$NAME.zip.sha256"
