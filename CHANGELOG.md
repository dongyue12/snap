# 更新日志

本项目的重要变更都记录在这里。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。

发布时，GitHub Release 的说明会从本文件里抽取对应版本的那一节，再附上产物的 SHA256。

## [未发布]

（下次发布的内容写在这里）

## [1.0.1] - 2026-09-12

这一版把"下载 → 装好 → 能用"的流程补齐了：发布包里直接带安装脚本，解压到 C 盘双击一下即可。

### 新增

- 发布包内加入 `add-to-path.bat`、`add-to-path.ps1`、`使用说明.txt`，与 `snap.exe` 同目录；
  解压后双击 `add-to-path.bat` 就能把这个目录加入 PATH
- `scripts/add-to-path.*`：把 snap 所在目录加入 PATH 的安装脚本
  - 默认只改**当前用户**的 PATH，不需要管理员；`/machine` 才动系统级（会自动检查权限）
  - 直接读写注册表，不用 `setx`（`setx` 在 PATH 超过 1024 字符时会截断）
  - 改前把原 PATH 备份到 `%TEMP%`；支持 `/remove` 撤销、`/dry-run` 先看后做
  - 写入后广播 `WM_SETTINGCHANGE`，新开的窗口立刻生效
- `使用说明.txt`：三步安装说明（放到 `C:\snap`、双击脚本、重开终端）与常见问题

### 变更

- 发布包的 Release 说明改为**从本文件抽取对应版本的小节**，再附上产物 SHA256
  （此前只有 GitHub 自动生成的一行 "Full Changelog"）
- `scripts/package.sh` 把 `CHANGELOG.md` 与安装脚本一并打进包
- README 的安装一节改为"解压到 `C:\snap` + 双击 `add-to-path.bat`"三步

### 修复

- `tests/scenario.sh` 在 Windows 反斜杠路径下会**死循环**：`$RUNNER_TEMP` 在 CI 里展开成
  `D:\a\_temp\...`，而向上查找外层仓库用的是 `while [ "$d" != "/" ]`，`dirname` 对反斜杠路径
  永远退不到 `/`。改成"父目录等于自己即停 + 层数上限"双保险
- `tests/scenario.sh` 依赖 GNU 专有命令（`md5sum`、`stat -c`），且写死了本机路径；
  改为自动探测哈希命令、外层仓库路径按条件判断
- `tests/scenario.sh` 硬要 `target/release` 产物，而 CI 只构建过 debug：
  现在按 `SNAP_BIN → release → debug → 自己构建` 依次查找
- CI 工作流的场景测试加 `timeout` 兜底，卡住时快速失败而不是耗到 job 超时

### 工程

- CI 收敛为 Windows 单平台（Linux/macOS 的配置留成注释，以后要开去掉即可），
  并在每次推送时产出一个可执行文件包
- CI 新增 `Release` 工作流：推送 `v*` 标签即自动检查、测试、打包、创建 Release；
  标签与 `Cargo.toml` 版本不一致会直接中止

## [1.0.0] - 2026-09-12

首个正式版本。

### 新增

- 整目录快照：`init` / `save` / `list` / `show` / `checkout` / `status` / `diff` / `gc`
- 三种等价写法：短选项（`snap -s`）、首字母（`snap s`）、完整子命令（`snap save`）
- 初始化时生成命令对照表（`.snap/命令对照表.md`），与 `--help` 同源
- `.snapignore` 忽略规则：目录规则（`logs/`）、任意层级（`*.log`）、锚定路径（`keep/c.txt`）、
  跨层级（`src/**/*.py`）
- `checkout` 三道保护：会丢内容的操作先列清单再确认；执行前预检（缺对象、路径被占、只读、
  无写权限）；中途失败不留半成品，只在全部成功时更新版本指针
- 内容寻址去重：相同内容全库只存一份，未改动的文件不重复占空间
- 跨平台：版本数据里的路径统一用正斜杠，仓库可在 Windows / Linux / macOS 之间复制
- `--version` / `-V` 输出包版本
- `scripts/package.sh`：本地一键打发布包（可执行文件 + 文档 + 许可证，含 SHA256）

### 性能

- **索引跳过未变文件**：记录大小、修改时间与内容哈希。15GB 目录首次扫描 10 秒，之后 0 秒
  （此前每次都要重新哈希全部内容）
- **按需压缩**：小于 4MB 用级别 6（源码/文本压得更小）；大文件先均匀取样判断可压缩性，
  省不到 10% 就直接存储——镜像、视频这类已压过的数据不再白烧 CPU
- **大文件分块并行**：8MB 以上按 4MB 分块交给多线程压缩，15GB 保存 17 秒（单线程时 165 秒）
- **全程流式读写**：内存占用与文件大小无关，11GB 单文件峰值 43MB
- 保存与扫描时在终端显示进度（管道/重定向时自动安静）

### 安全

- 索引除大小、修改时间外还记录 **ctime**，并按 git 的 racy 规则判定：即使有工具在改内容的
  同时把修改时间改回去（`touch -r`、`cp -p`、`rsync -t`），改动也不会被漏掉
- 索引写入失败不影响结果（只读文件系统上 `status` 仍可用）；`SNAP_NO_INDEX=1` 可完全禁用索引
- 切换版本时若读不到确认输入（CI/管道），按**取消**处理，绝不擅自覆盖
- 场景测试脚本拒绝重建含有非脚本内容的目录，避免误删用户数据

### 文档

- `README.md`：快速开始、命令表、安全机制、性能实测、有意不做的部分、发布流程
- `rust/snap/README.md`：磁盘格式说明、压缩策略的取舍依据、与同类工具的差异
- 命令对照表随 `init` 生成在 `.snap/命令对照表.md`

### 测试

- 39 项 cargo 测试（8 单元 + 31 命令行集成）
- 108 项场景测试（`rust/snap/tests/scenario.sh`，在真实文件上按使用顺序跑一遍）
- CI：Windows 平台跑格式检查、`clippy -D warnings`、测试、场景测试，并产出可执行文件包

[未发布]: https://github.com/dongyue12/snap/compare/v1.0.1...HEAD
[1.0.1]: https://github.com/dongyue12/snap/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/dongyue12/snap/releases/tag/v1.0.0
