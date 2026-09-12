# snap

[![CI](https://github.com/dongyue12/snap/actions/workflows/ci.yml/badge.svg)](https://github.com/dongyue12/snap/actions/workflows/ci.yml)

极简的项目版本快照工具 —— **给一个目录整目录打快照，存一个版本、切回一个版本**。
不做分支、不做合并、不连远程仓库，只有"存"和"切"这两件事。

*Minimal directory snapshot tool. One command to save a version, one to switch back.*

- **一个可执行文件**，无运行时依赖，约 570 KB（Release 构建，strip + LTO）
- **数据就在项目里**：所有版本放在 `.snap/` 目录内，复制走它 = 备份了全部历史
- **内容寻址去重**：相同内容全库只存一份，没改动的文件不重复占空间
- **切换有保护**：会丢内容的操作先列清单再确认，执行前预检，不留半成品
- **大文件不慌**：流式读写（内存与文件大小无关）+ 多线程分块压缩
- 跨平台（Windows / Linux / macOS），仓库可在系统之间复制

## 安装

从 [Releases](https://github.com/dongyue12/snap/releases) 下载 `snap-<版本>-<平台>.zip`。

**推荐三步**：

1. 解压到 **`C:\snap`**（路径尽量别带空格和中文，最省事）。包里已经带了安装脚本：
   `snap.exe`、`add-to-path.bat`、`add-to-path.ps1`、`使用说明.txt`、`README.md`、`CHANGELOG.md`、`LICENSE`。
2. **双击 `add-to-path.bat`** —— 它把 `C:\snap` 加进「当前用户」的 PATH（不需要管理员），
   改前会自动把原 PATH 备份到 `%TEMP%`。
3. **重新开一个终端窗口**，之后在任何目录都能直接敲 `snap`。

不想装 PATH 也行：在项目目录里执行 `C:\snap\snap.exe -t` 即可。

安装脚本的其它用法：

| 操作 | 命令 |
| --- | --- |
| 先看会改什么，不真正修改 | `add-to-path.bat /dry-run` |
| 加入系统 PATH（所有用户，需管理员） | 右键以管理员身份运行 `add-to-path.bat /machine` |
| 从 PATH 移除 | `add-to-path.bat /remove` |

也可以直接用 PowerShell：`add-to-path.ps1 -Scope User|Machine -Remove -DryRun`。
脚本默认只改当前用户，不动系统级设置；用的是直接读写注册表，不用 `setx`
（`setx` 在 PATH 超过 1024 字符时会截断）。

| 操作 | 命令 |
| --- | --- |
| 加入当前用户 PATH（不需要管理员） | `add-to-path.bat` |
| 加入系统 PATH（需要管理员） | `add-to-path.bat /machine` |
| 先看会改什么，不真正修改 | `add-to-path.bat /dry-run` |
| 从 PATH 移除 | `add-to-path.bat /remove` |

也可以直接用 PowerShell：`scripts/add-to-path.ps1 -Scope User|Machine -Remove -DryRun`。

## 快速开始

```bash
cd rust/snap
cargo build --release          # 产物: target/release/snap.exe（Linux/macOS 为 snap）

cd /path/to/your-project
snap -i                        # 初始化，生成 .snap 与命令对照表
snap -t                        # 看改了什么：+ 新增 / M 修改 / - 删除
snap -s "完成登录功能"           # 存一个版本
snap -l                        # 看历史，带 * 的是当前版本
snap -c a55b                   # 切回某个版本（写前几位前缀即可）
```

## 命令

短写法、首字母、完整写法三者完全等价：

| 想做的事 | 短写法 | 首字母 | 完整写法 |
| --- | --- | --- | --- |
| 初始化当前目录 | `snap -i` | `snap i` | `snap init` |
| 查看工作区状态 | `snap -t` | `snap t` | `snap status` |
| 保存当前状态为新版本 | `snap -s "说明"` | `snap s "说明"` | `snap save "说明"` |
| 列出所有版本 | `snap -l` | `snap l` | `snap list` |
| 查看版本详情 | `snap -v <版本>` | `snap v <版本>` | `snap show <版本>` |
| 切换到指定版本 | `snap -c <版本>` | `snap c <版本>` | `snap checkout <版本>` |
| 比较差异 | `snap -d <版本> [版本2]` | `snap d <版本>` | `snap diff <版本> [版本2]` |
| 清理未被引用的对象 | `snap -g` | `snap g` | `snap gc` |

在任意子目录里都能用（会向上找到项目根的 `.snap`）。初始化后完整对照表在
`.snap/命令对照表.md`；`snap -h` 看帮助，`snap --version` 看版本。

## 切换版本时的三道保护

`checkout` 会覆盖文件，所以：

1. **会丢内容先确认**：工作区里有没保存到任何版本的内容时，先列出清单再问
   `继续? [y/N]`；回车或 `n` 就取消，一个文件都不会动。在管道/CI 里读不到确认输入时
   **按取消处理**，绝不替你猜 `y`。
2. **执行前预检**：对象库缺对象、目标路径被同名目录占住、上层路径是文件、文件只读、
   目录无写权限——一次性列出来，**在动任何文件之前**报错退出。
3. **不留半成品**：只有全部成功才更新版本指针；中途失败会明确报告哪些文件失败，
   并提示重新运行。

## 忽略规则（`.snapignore`）

```
*.log          不含 "/" 的模式，匹配任意一层里的同名条目
logs/          以 "/" 结尾表示目录规则，匹配该目录及其下所有内容
keep/c.txt     含 "/" 的模式相对项目根目录定位，"*" 不跨目录层级
src/**/*.py    "**" 可跨任意层级
# 开头的行和空行忽略；不支持 "!" 反选
```

默认忽略 `.snap/` `.vsnap/` `.git/` `__pycache__/` `*.pyc` `*.pyo` `.DS_Store` `*.swp` `*~`；
**正在运行的程序文件自己也不会进版本库**，所以把 `snap.exe` 放在项目里也没关系。

## 性能（32GB 内存 / SSD / Windows 实测）

| 操作 | 数据量 | 耗时 | 峰值内存 |
| --- | --- | --- | --- |
| `-t` 首次扫描（全量哈希） | 15 GB | 10 秒 | 很小 |
| `-t` 再次扫描（索引跳过未变文件） | 15 GB | **0 秒** | 很小 |
| `-s` 保存（8 线程分块压缩） | 15 GB | 17 秒 | 43 MB |
| `-c` 切回（解压 + 写文件） | 1.4 GB 单文件 | 3 秒 | 43 MB |

设计要点：**索引**（大小 + 修改时间 + ctime，判定规则参照 git 的索引）、
**按需压缩**（小文件高压缩比、压不动的大文件直接存储）、**大文件分块并行**。
依据与取舍都写在 [rust/snap/README.md](rust/snap/README.md) 里。

## 项目结构

```
.
├── rust/snap/           Rust 实现
│   ├── src/             main(入口/命令) store(存储/索引) chunk(分块压缩)
│   │                    ignore(忽略规则) docs(帮助/对照表) winout(输出)
│   └── tests/           cli.rs(集成测试) scenario.sh(场景测试) 沙箱说明
├── test/                测试沙箱：形态各异的 fixture + iso/(性能测试用镜像)
└── LICENSE              MIT
```

## 开发与测试

```bash
cd rust/snap
cargo fmt --check              # 格式
cargo clippy -- -D warnings    # 静态检查（当前 0 告警）
cargo test                     # 38 项：8 单元 + 30 命令行集成
SNAP_SANDBOX=/tmp/snap-box bash tests/scenario.sh   # 109 项场景检查
```

## 有意不做的部分

为了保持"极简"，这些东西**故意没有**：分支与合并、远程仓库、暂存区、reflog、
增量（delta）存储、文件锁。历史是线性的；`gc` 在正常使用中基本无事可做，
只在对象库有残留时起作用。适用的场景是"一个人给一个项目做本地快照"，
不适合多人协作或需要历史改写的场合。

## 发布

本地打包（Windows 上用 git-bash）：

```bash
bash scripts/package.sh        # 产出 dist/snap-<版本>-<目标平台>.zip 与 .sha256
```

自动发布：推一个标签即可，GitHub Actions 会跑完检查、打包并创建 Release
（标签必须与 `Cargo.toml` 里的版本一致，否则会中止）：

```bash
# 先把 Cargo.toml 里的 version 改成 1.0.1
git commit -am "发布 1.0.1"
git tag v1.0.1
git push && git push --tags
```

## 更新日志

见 [CHANGELOG.md](CHANGELOG.md)。

## 许可证

[MIT](LICENSE) © 2026 dongyue
