#!/usr/bin/env bash
# snap 场景测试：在沙箱目录里重建一套文件，然后把所有命令和边界情况跑一遍。
#
# 用法（需要 git-bash / bash）:
#     bash rust/snap/tests/scenario.sh
#     SNAP_BIN=path/to/snap.exe SNAP_SANDBOX=path/to/sandbox bash .../scenario.sh
#
# 注意：脚本会把沙箱目录整个删掉重建，所以不要指向有真实内容的目录。

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# 找可执行文件：优先 SNAP_BIN，其次 target/release，再退到 target/debug，
# 都没有就自己构建（CI 上 cargo test 只产出 debug 产物，没有 release）
find_bin() {
  if [ -n "${SNAP_BIN:-}" ]; then printf '%s' "$SNAP_BIN"; return; fi
  for cand in "$HERE/../target/release/snap.exe" "$HERE/../target/release/snap"               "$HERE/../target/debug/snap.exe" "$HERE/../target/debug/snap"; do
    if [ -x "$cand" ]; then printf '%s' "$cand"; return; fi
  done
  printf ''
}

BIN="$(find_bin)"
if [ -z "$BIN" ]; then
  echo "没找到 snap 可执行文件，正在构建 release（cargo build --release --locked）..."
  ( cd "$HERE/.." && cargo build --release --locked ) || { echo "构建失败，中止" >&2; exit 2; }
  BIN="$(find_bin)"
fi
if [ -z "$BIN" ]; then
  echo "仍然找不到可执行文件；请用 SNAP_BIN=/path/to/snap 指定" >&2
  exit 2
fi
echo "使用可执行文件: $BIN"
W="${SNAP_SANDBOX:-$HERE/../../../test}"

PASS=0; FAIL=0; FAILED=()
ok()  { PASS=$((PASS+1)); printf '  [PASS] %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); FAILED+=("$1"); printf '  [FAIL] %s   :: %s\n' "$1" "$2"; }
chk() { if [ "$1" = "1" ]; then ok "$2"; else bad "$2" "$3"; fi; }
has()  { case "$2" in *"$1"*) echo 1;; *) echo 0;; esac; }
nhas() { case "$2" in *"$1"*) echo 0;; *) echo 1;; esac; }
run()  { (cd "$W" && "$BIN" "$@" 2>&1); }
rcy()  { (cd "$W" && printf 'y\n' | "$BIN" "$@" 2>&1); }     # 对可能出现的确认回答 y
rcn()  { (cd "$W" && printf 'n\n' | "$BIN" "$@" 2>&1); }     # 回答 n
rc_of(){ (cd "$W" && "$BIN" "$@" >/dev/null 2>&1); echo $?; }
headid(){ cat "$W/.snap/HEAD"; }
gid()  { printf '%s' "$1" | grep -o '[0-9a-f]\{12\}' | head -1; }
# 哈希与取大小：Linux 用 md5sum/stat，macOS 用 md5/stat -f，都没有就退回 cksum。
# （CI 会在三个平台跑，所以这里不能依赖 GNU 专有命令）
if command -v md5sum >/dev/null 2>&1; then
  hashf() { md5sum | cut -d' ' -f1; }
elif command -v md5 >/dev/null 2>&1; then
  hashf() { md5 -q; }
else
  hashf() { cksum | cut -d' ' -f1; }
fi
md5f() { hashf < "$W/$1"; }
size_of() { wc -c < "$1" | tr -d ' 
'; }

# ---------------------------------------------------------------- 沙箱
MARKER=".snap-sandbox"          # 标记文件：只有本脚本建的沙箱才允许重建

# 防止把用户自己的数据删掉：目录里已经有别的内容、又没有标记时，直接中止。
guard_sandbox() {
  [ -d "$W" ] || return 0
  [ -f "$W/$MARKER" ] && return 0
  [ -z "$(ls -A "$W" 2>/dev/null)" ] && return 0
  printf '\n沙箱目录里已经有内容，而且不是本脚本建的：\n    %s\n' "$W" >&2
  printf '为避免误删你的文件，脚本已中止。\n' >&2
  printf '如果确实要重建，请先移走里面的内容，或手动创建标记文件：\n    touch "%s/%s"\n' "$W" "$MARKER" >&2
  printf '也可以用 SNAP_SANDBOX=/别的目录 指定另一个沙箱。\n\n' >&2
  exit 2
}

build_fixture() {
  guard_sandbox
  rm -rf "$W"
  mkdir -p "$W"/{src/lib/deep/nested,docs/zh,bin/logs,logs/deep,tmp,keep,json,build,__pycache__}
  : > "$W/$MARKER"
  printf '# 测试项目\n\n这是 snap 的功能测试沙箱。\n'      > "$W/README.md"
  # 沙箱说明（仓库里有更详细的一份，直接带进来）
  [ -f "$HERE/sandbox-README.md" ] && cp "$HERE/sandbox-README.md" "$W/README.md"
  printf 'fn main() {\n    println!("hello");\n}\n'        > "$W/src/main.rs"
  printf 'pub fn add(a: i32, b: i32) -> i32 { a + b }\n'   > "$W/src/lib/util.rs"
  printf 'pub const X: i32 = 1;\n'                         > "$W/src/lib/deep/nested/mod.rs"
  printf '# 指南\n'                                        > "$W/docs/guide.md"
  printf '中文文件名的文档\n'                                > "$W/docs/zh/说明.md"
  printf '空格\n'                                          > "$W/带空格 的文件.txt"
  printf '中文内容\n'                                       > "$W/中文名.txt"
  printf 'emoji\n'                                         > "$W/emoji-🎉.txt"
  printf 'dash\n'                                          > "$W/dash-start.txt"
  printf '大写\n'                                          > "$W/UPPER.TXT"
  head -c 65536 /dev/urandom                               > "$W/bin/random.dat"
  for i in $(seq 0 255); do printf "\\x$(printf %02x $i)"; done > "$W/bin/all-bytes.bin"
  printf ''                                                > "$W/empty.txt"
  printf '\n'                                              > "$W/only-newline.txt"
  printf 'a\r\nb\r\n'                                      > "$W/crlf.txt"
  printf 'with bom\n'                                      > "$W/utf8-bom.txt"
  printf '{"name": "snap", "debug": true}\n'               > "$W/json/config.json"
  # 应当被忽略的
  printf '# 忽略规则\nlogs/\n*.log\ntmp/\n*.tmp\n'          > "$W/.snapignore"
  printf '日志噪音\n'                                        > "$W/logs/app.log"
  printf '日志噪音\n'                                        > "$W/logs/deep/nested.log"
  printf '临时\n'                                            > "$W/tmp/scratch.tmp"
  printf '噪音\n'                                            > "$W/debug.log"
  printf '保留\n'                                            > "$W/keep/note.md"
  # 由用户自己决定是否忽略的
  printf 'SECRET=1\n'                                      > "$W/.env"
  printf 'BUILD\n'                                          > "$W/build/out.bin"
  printf 'PYC\n'                                            > "$W/__pycache__/x.pyc"
}

echo "================================================================"
echo "0. 重建沙箱"
echo "================================================================"
build_fixture
echo "  test/ 已重建：$(cd "$W" && find . -type f | wc -l) 个文件"
echo "  二进制快照: all-bytes=$(md5f bin/all-bytes.bin | cut -c1-8) random=$(md5f bin/random.dat | cut -c1-8)"
BIN_MD5="$(md5f bin/all-bytes.bin)"; RAND_MD5="$(md5f bin/random.dat)"

echo
echo "================================================================"
echo "1. init（嵌套仓库）"
echo "================================================================"
out=$(run -i)
chk "$(has '初始化完成' "$out")" "init 成功" "$out"
# 沙箱在别的仓库里才会提示"嵌套仓库"，独立位置则不提示
# 向上找外层仓库。这里不能用 while [ "$d" != "/" ]：
# Windows 上 $W 可能是 D:\_temp\... 这种反斜杠形式，dirname 永远退不到 "/"，
# 会变成死循环（CI 上就卡在这里）。改成"父目录等于自己就停" + 层数上限双保险。
NESTED=0; d="$W"; depth=0
while :; do
  depth=$((depth + 1))
  [ "$depth" -gt 16 ] && break
  parent="$(dirname "$d")"
  [ "$parent" = "$d" ] && break
  d="$parent"
  if [ -d "$d/.snap" ]; then NESTED=1; break; fi
done
if [ "$NESTED" = "1" ]; then
  chk "$(has '嵌套仓库' "$out")" "提示成为嵌套仓库" "$out"
else
  chk "$(nhas '嵌套仓库' "$out")" "独立位置初始化不提嵌套" "$out"
fi
chk "$([ -d "$W/.snap" ] && echo 1)" "生成 .snap 目录" ""
chk "$([ -f "$W/.snap/命令对照表.md" ] && echo 1)" "生成命令对照表" ""
chk "$(has '`snap -i`' "$(cat "$W/.snap/命令对照表.md")")" "对照表内容正确" ""
out=$(run -i)
chk "$(has '已经初始化过了' "$out")" "重复 init 反应正确" "$out"

echo
echo "================================================================"
echo "2. status 与忽略规则（首次）"
echo "================================================================"
out=$(run -t)
chk "$(has '当前版本: (无)' "$out")" "无版本时提示正确" "$out"
chk "$(has '+ README.md' "$out")" "普通文件被列出" "$out"
chk "$(nhas 'logs/' "$out")" "目录规则 logs/ 生效" "$out"
chk "$(nhas 'debug.log' "$out")" "*.log 任意层级生效" "$out"
chk "$(nhas 'scratch.tmp' "$out")" "目录规则 tmp/ 生效" "$out"
chk "$(nhas 'x.pyc' "$out")" "默认忽略 __pycache__/" "$out"
chk "$(has '+ .env' "$out")" "点文件默认会被跟踪（.env）" "$out"
chk "$(has '+ build/out.bin' "$out")" "未列入忽略的 build/ 会被跟踪" "$out"
chk "$(has '+ emoji-🎉.txt' "$out")" "emoji 文件名被列出" "$out"
chk "$(has '+ docs/zh/说明.md' "$out")" "中文路径被列出" "$out"

echo
echo "================================================================"
echo "3. save / list / show"
echo "================================================================"
out=$(run -s "第一个版本")
V1=$(gid "$out")
chk "$(has '已保存' "$out")" "save 成功" "$out"
chk "$([ -n "$V1" ] && [ "$(headid)" = "$V1" ] && echo 1)" "HEAD 指向新版本" "$(headid) vs $V1"
chk "$(has '个文件' "$out")" "输出包含文件数" "$out"
out=$(run -t)
chk "$(has '工作区干净' "$out")" "保存后工作区干净" "$out"
out=$(run -s "重复保存")
chk "$(has '无需保存' "$out")" "相同内容不产生新版本" "$out"
chk "$([ "$(ls "$W/.snap/versions" | wc -l)" = "1" ] && echo 1)" "版本文件仍只有 1 个" "$(ls "$W/.snap/versions")"

out=$(run -l)
chk "$(has "* $V1" "$out")" "list 标记 HEAD" "$out"
out=$(run -v "${V1:0:4}")
chk "$(has "版本:   $V1" "$out")" "show 支持前缀" "$out"
chk "$(has 'README.md' "$out")" "show 列出文件清单" "$out"
chk "$(has 'src/lib/deep/nested/mod.rs' "$out")" "嵌套路径用正斜杠" "$out"
chk "$(nhas '\\' "$out")" "show 输出无反斜杠" "$out"
chk "$(has '.env' "$out")" "点文件确实进了版本" "$out"
out=$(run -v deadbeef)
chk "$(has '找不到版本' "$out")" "不存在的版本友好报错" "$out"
chk "$(nhas 'panicked' "$out")" "不存在的版本不崩溃" "$out"
out=$(run -v "")
chk "$(has '版本ID' "$out")" "空前缀被拒绝" "$out"

echo
echo "================================================================"
echo "4. 改动检测 / diff / 第二个版本"
echo "================================================================"
printf 'fn main() {\n    println!("changed");\n}\n' > "$W/src/main.rs"
printf '新内容\n' > "$W/new-file.txt"
rm -f "$W/keep/note.md"
out=$(run -t)
chk "$(has 'M src/main.rs' "$out")" "status 报修改" "$out"
chk "$(has '+ new-file.txt' "$out")" "status 报新增" "$out"
chk "$(has '- keep/note.md' "$out")" "status 报删除" "$out"
out=$(run -d "${V1:0:4}")
chk "$(has 'M src/main.rs' "$out")" "diff 工作区：修改" "$out"
chk "$(has '+ new-file.txt' "$out")" "diff 工作区：新增" "$out"
chk "$(has '- keep/note.md' "$out")" "diff 工作区：删除" "$out"
out=$(run -s "第二个版本")
V2=$(gid "$out")
chk "$(has '已保存' "$out")" "保存第二个版本" "$out"
out=$(run -d "${V1:0:4}" "${V2:0:4}")
chk "$(has 'M src/main.rs' "$out")" "diff 两个版本" "$out"
out=$(run -l)
chk "$(has "* $V2" "$out")" "list 的 HEAD 已移到第二版" "$out"
chk "$(has "<-${V1:0:6}" "$out")" "list 显示父版本" "$out"

echo
echo "================================================================"
echo "5. checkout 还原能力（含二进制、空文件、特殊文件名）"
echo "================================================================"
printf 'mutated' > "$W/bin/all-bytes.bin"
printf 'mutated' > "$W/bin/random.dat"
printf 'not empty' > "$W/empty.txt"
printf 'x' > "$W/emoji-🎉.txt"
printf 'y' > "$W/带空格 的文件.txt"
printf 'z' > "$W/中文名.txt"
out=$(rcn -c "${V1:0:4}")      # 有未保存改动 → 先答 n
chk "$(has '警告' "$out")" "有未保存内容时先警告" "$out"
chk "$(has 'bin/all-bytes.bin' "$out")" "列出会丢的文件" "$out"
out=$(rcy -c "${V1:0:4}")      # 答 y 执行
chk "$(has '已切换到' "$out")" "checkout 成功" "$out"
chk "$([ "$(md5f bin/all-bytes.bin)" = "$BIN_MD5" ] && echo 1)" "二进制文件逐字节还原" "$(md5f bin/all-bytes.bin)"
chk "$([ "$(md5f bin/random.dat)" = "$RAND_MD5" ] && echo 1)" "随机二进制还原" "$(md5f bin/random.dat)"
chk "$([ "$(size_of "$W/empty.txt")" = "0" ] && echo 1)" "空文件还原为空" "$(size_of "$W/empty.txt")"
chk "$(has 'emoji' "$(cat "$W/emoji-🎉.txt")")" "emoji 文件名还原" "$(cat "$W/emoji-🎉.txt")"
chk "$(has '空格' "$(cat "$W/带空格 的文件.txt")")" "带空格文件名还原" "$(cat "$W/带空格 的文件.txt")"
chk "$(has '中文内容' "$(cat "$W/中文名.txt")")" "中文文件名还原" "$(cat "$W/中文名.txt")"
chk "$([ -f "$W/keep/note.md" ] && echo 1)" "被删文件被还原" ""
chk "$([ ! -f "$W/new-file.txt" ] && echo 1)" "新增文件被移除" ""
out=$(run -t)
chk "$(has '工作区干净' "$out")" "checkout 后工作区干净" "$out"

echo
echo "================================================================"
echo "6. checkout 安全机制（三道保护）"
echo "================================================================"
printf '用户手写的内容（从未保存）\n' > "$W/README.md"
out=$(rcn -c "${V1:0:4}")
chk "$(has '警告' "$out")" "覆盖前给出警告" "$out"
chk "$(has 'README.md' "$out")" "列出会丢的文件" "$out"
chk "$(has '用户手写的内容' "$(cat "$W/README.md")")" "答 n 时内容保留" "$(cat "$W/README.md")"
out=$(run -c "${V1:0:4}")                       # stdin 关闭
chk "$(has '无法读取确认输入' "$out")" "无 stdin 时按取消处理" "$out"
chk "$(has '用户手写的内容' "$(cat "$W/README.md")")" "无 stdin 时不覆盖" "$(cat "$W/README.md")"
out=$(rcy -c "${V1:0:4}")
chk "$(has '已切换到' "$out")" "答 y 才覆盖" "$out"
chk "$(nhas '用户手写的内容' "$(cat "$W/README.md")")" "答 y 后确实被覆盖" "$(cat "$W/README.md")"

printf '本地未保存修改\n' > "$W/README.md"
out=$(rcn -c "${V1:0:4}")
chk "$(has '警告' "$out")" "HEAD==目标版本时也会警告" "$out"
chk "$(has '本地未保存修改' "$(cat "$W/README.md")")" "答 n 时修改保留" "$(cat "$W/README.md")"
rcy -c "${V1:0:4}" >/dev/null

echo "--- 预检：未跟踪文件挡住目标版本需要的目录 ---"
rcy -c "${V2:0:4}" >/dev/null                 # 回到干净基线
VA=$(headid)                                  # 基线版本：树里没有 cfg 条目
out=$(run -t)
chk "$(has '工作区干净' "$out")" "基线工作区干净" "$out"
mkdir -p "$W/cfg" && printf 'inner\n' > "$W/cfg/inner.txt"
out=$(run -s "cfg 是目录"); VB2=$(gid "$out")
chk "$([ -n "$VB2" ] && echo 1)" "记录了 cfg 是目录的版本" "$out"
# 磁盘上把 cfg 目录换成未跟踪的文件，再切回没有 cfg 条目的版本：
# 于是 cfg 不属于任何版本、不会被删除，但目标版本需要它是个目录
rm -rf "$W/cfg" && printf '未跟踪的挡路文件\n' > "$W/cfg"
rcy -c "${VA:0:4}" >/dev/null
chk "$([ -f "$W/cfg" ] && [ "$(headid)" = "$VA" ] && echo 1)" "构造好挡路场景" "cfg是文件=$([ -f "$W/cfg" ] && echo yes || echo no) HEAD=$(headid) VA=$VA"
BEFORE="$(cd "$W" && find . -not -path './.snap/*' | sort | hashf)"
out=$(rcy -c "${VB2:0:4}")
chk "$(has '需要先把它移走' "$out")" "预检指出挡路的文件" "$out"
chk "$(has 'cfg' "$out")" "预检点名了挡路的路径" "$out"
chk "$(nhas '已切换到' "$out")" "预检拦下冲突" "$out"
chk "$(nhas 'panicked' "$out")" "预检失败不崩溃" "$out"
AFTER="$(cd "$W" && find . -not -path './.snap/*' | sort | hashf)"
chk "$([ "$BEFORE" = "$AFTER" ] && echo 1)" "预检失败时磁盘原样" ""
chk "$([ "$(headid)" = "$VA" ] && echo 1)" "预检失败时 HEAD 不变" "$(headid) vs $VA"
chk "$([ "$(cat "$W/cfg")" = '未跟踪的挡路文件' ] && echo 1)" "挡路的未跟踪文件没被动过" "$(cat "$W/cfg")"
rm -f "$W/cfg"
out=$(rcy -c "${VB2:0:4}")
chk "$(has '已切换到' "$out")" "移走挡路文件后即可切换" "$out"

echo
echo "================================================================"
echo "7. 文件↔目录 互换"
echo "================================================================"
printf 'v-file\n' > "$W/swapd"
out=$(run -s "swapd 是文件"); VS1=$(gid "$out")
rm -f "$W/swapd" && mkdir -p "$W/swapd" && printf 'v-dir\n' > "$W/swapd/inner.txt"
out=$(run -s "swapd 是目录"); VS2=$(gid "$out")
out=$(rcy -c "${VS1:0:4}")
chk "$(has '已切换到' "$out")" "目录换成文件成功" "$out"
chk "$([ -f "$W/swapd" ] && [ "$(cat "$W/swapd")" = 'v-file' ] && echo 1)" "swapd 已是文件且内容正确" "$(cat "$W/swapd" 2>/dev/null)"
chk "$([ ! -d "$W/swapd" ] && echo 1)" "旧目录已清理" ""
out=$(rcy -c "${VS2:0:4}")
chk "$(has '已切换到' "$out")" "文件换成目录成功" "$out"
chk "$([ -f "$W/swapd/inner.txt" ] && echo 1)" "目录已重建" ""

echo
echo "================================================================"
echo "8. 深层目录与空目录回收"
echo "================================================================"
rcy -c "${V2:0:4}" >/dev/null
mkdir -p "$W/deep/a/b/c" && printf 'deep\n' > "$W/deep/a/b/c/x.txt"
out=$(run -s "加一个深层目录"); VD=$(gid "$out")
rm -rf "$W/deep"
out=$(run -s "删掉深层目录"); VE=$(gid "$out")
out=$(rcy -c "${VD:0:4}")
chk "$([ -f "$W/deep/a/b/c/x.txt" ] && echo 1)" "深层文件被重建" ""
out=$(rcy -c "${VE:0:4}")
chk "$([ ! -d "$W/deep" ] && echo 1)" "空目录被回收" ""

echo
echo "================================================================"
echo "9. gc / 损坏数据 / 边界"
echo "================================================================"
mkdir -p "$W/.snap/objects/zz" && printf 'garbage' > "$W/.snap/objects/zz/notahash"
out=$(run -g)
chk "$(has '清理了' "$out")" "gc 可执行" "$out"
chk "$([ ! -f "$W/.snap/objects/zz/notahash" ] && echo 1)" "gc 清掉孤儿对象" ""
out=$(run -l)
chk "$(has "$V1" "$out")" "gc 后历史完好" "$out"
out=$(run -t)
chk "$(has '工作区干净' "$out")" "gc 后仓库自洽" "$out"

printf '{ 这不是 json' > "$W/.snap/versions/zzzzzzzzzzzz.json"
rc=$(rc_of -l)
chk "$([ "$rc" = "0" ] && echo 1)" "坏版本文件不影响 list" "rc=$rc"
rm -f "$W/.snap/versions/zzzzzzzzzzzz.json"

# 对象缺失：应友好报错且不动工作区
IDX=$(headid)
H=$(grep -o "\"README\.md\": \"[0-9a-f]*\"" "$W/.snap/versions/$IDX.json" | grep -o '[0-9a-f]\{40\}' | head -1)
if [ -n "$H" ]; then
  mv "$W/.snap/objects/${H:0:2}/${H:2}" "$W/.snap/objects/${H:0:2}/${H:2}.bak"
  printf '本地改动\n' > "$W/README.md"          # 让这个文件确实需要被写入
  BEFORE="$(cd "$W" && find . -not -path './.snap/*' | sort | hashf)"
  out=$(rcy -c "${IDX:0:4}")
  chk "$(has '缺少对象' "$out")" "对象缺失给出可读错误" "$out"
  chk "$(nhas 'panicked' "$out")" "对象缺失不崩溃" "$out"
  chk "$(nhas '已切换到' "$out")" "对象缺失时不更新 HEAD" "$out"
  AFTER="$(cd "$W" && find . -not -path './.snap/*' | sort | hashf)"
  chk "$([ "$BEFORE" = "$AFTER" ] && echo 1)" "对象缺失时磁盘原样" ""
  mv "$W/.snap/objects/${H:0:2}/${H:2}.bak" "$W/.snap/objects/${H:0:2}/${H:2}"
  rcy -c "${IDX:0:4}" >/dev/null 2>&1
else
  bad "取到 README.md 的对象哈希" "$IDX"
fi

echo
echo "================================================================"
echo "10. 短写法 / 别名 / 帮助 / 自排除 / 只读文件"
echo "================================================================"
a=$(run -t); b=$(run status); c=$(run t)
chk "$([ "$a" = "$b" ] && [ "$b" = "$c" ] && echo 1)" "status / -t / t 输出一致" ""
rc=$(rc_of); chk "$([ "$rc" != "0" ] && echo 1)" "无参数退出非零" "rc=$rc"
chk "$(has 'usage:' "$(run)")" "无参数输出用法" ""
rc=$(rc_of --help); chk "$([ "$rc" = "0" ] && echo 1)" "--help 正常" "rc=$rc"
rc=$(rc_of bogus); chk "$([ "$rc" != "0" ] && echo 1)" "未知子命令报错" "rc=$rc"
rc=$(rc_of -c); chk "$([ "$rc" != "0" ] && echo 1)" "checkout 缺参数报错" "rc=$rc"

cp "$BIN" "$W/snap.exe"
printf '\n改动一下，确保会产生新版本\n' >> "$W/README.md"
out=$(cd "$W" && ./snap.exe -s "自排除测试" 2>&1)
VS=$(gid "$out")
if [ -n "$VS" ]; then
  TREE=$(cat "$W/.snap/versions/$VS.json")
  chk "$(has '已保存' "$out")" "用复制进来的 exe 能正常保存" "$out"
  chk "$(nhas 'snap.exe' "$TREE")" "程序自己不进版本" "树里出现了 snap.exe"
  chk "$(has 'README.md' "$TREE")" "其他文件正常收录" ""
  chk "$(nhas 'snap.exe' "$(grep snap.exe "$W/.snap/versions/$VS.json")")" "版本文件里没有 snap.exe" ""
else
  bad "自排除测试可保存" "$out"
fi
rm -f "$W/snap.exe"

# 只读文件：预检应拦下
printf 'v1\n' > "$W/ro.txt"
out=$(run -s "只读测试一"); VR1=$(gid "$out")
printf 'v2\n' > "$W/ro.txt"
out=$(run -s "只读测试二"); VR2=$(gid "$out")
chmod a-w "$W/ro.txt"
out=$(rcy -c "${VR1:0:4}")
chmod u+w "$W/ro.txt"
chk "$(has '只读' "$out")" "只读文件被预检拦下" "$out"
chk "$(nhas 'panicked' "$out")" "只读文件不崩溃" "$out"
chk "$([ "$(headid)" = "$VR2" ] && echo 1)" "只读时 HEAD 不变" "$(headid)"
rcy -c "${VR2:0:4}" >/dev/null 2>&1

echo
echo "================================================================"
echo "11. 从子目录使用 / 外层仓库不受影响"
echo "================================================================"
mkdir -p "$W/src/lib/deep"
out=$(cd "$W/src/lib/deep" && "$BIN" -t 2>&1)
chk "$(has '当前版本' "$out")" "子目录里能识别仓库" "$out"
out=$(cd "$W/src/lib/deep" && "$BIN" -l 2>&1)
chk "$(has "$V1" "$out")" "子目录里能列出历史" "$out"

# 外层仓库：只在沙箱上一层真的是 snap 仓库时才检查（CI 里没有这层，跳过）
OUTER="$(cd "$W/.." && pwd)"
if [ -d "$OUTER/.snap" ]; then
  o=$(cd "$OUTER" && "$BIN" -t 2>&1)
  chk "$(nhas 'test/.snap/' "$o")" "外层忽略内层数据目录" "$o"
  chk "$(nhas '+ test/' "$o")" "外层把整个 test/ 忽略掉（.snapignore 那条）" "$o"
else
  echo "  (跳过：沙箱上一层没有 snap 仓库)"
fi

echo
echo "================================================================"
echo "11b. 索引加固：换了内容但保留修改时间，也必须能发现"
echo "================================================================"
REF="/tmp/snap-ref-$$"
cp "$W/src/main.rs" "$REF"
printf 'fn main() { /* 被改过，但修改时间被还原 */ }
' > "$W/src/main.rs"
touch -r "$REF" "$W/src/main.rs"      # 模拟 cp -p / touch -r / rsync -t
out=$(run -t)
chk "$(has 'M src/main.rs' "$out")" "保留修改时间的改动也能发现" "$out"
rm -f "$REF"
rcy -c "$(headid)" >/dev/null 2>&1     # 还原工作区，便于后面的收尾检查

echo
echo "================================================================"
echo "12. 收尾：工作区应干净"
echo "================================================================"
out=$(run -t)
chk "$(has '工作区干净' "$out")" "沙箱最终状态干净" "$out"

echo
echo "================================================================"
printf '总计 %d 项通过, %d 项失败\n' "$PASS" "$FAIL"
if [ "$FAIL" -gt 0 ]; then
  printf '失败项:\n'
  for f in "${FAILED[@]}"; do printf '  - %s\n' "$f"; done
fi
echo "================================================================"
exit $((FAIL > 0))
