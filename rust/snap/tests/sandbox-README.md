# snap 测试沙箱

这个目录是用来在**真实文件**上验证 snap 的，里面的文件是特意准备的各种形态：

| 类别 | 内容 |
| --- | --- |
| 多层嵌套 | `src/lib/deep/nested/mod.rs`、`docs/zh/说明.md` |
| 文件名形态 | 中文、emoji（`emoji-🎉.txt`）、带空格、大写 `UPPER.TXT`、以 `-` 开头 |
| 二进制 | `bin/all-bytes.bin`（0..255 全部字节）、`bin/random.dat`（64KB 随机） |
| 边界内容 | `empty.txt`（空）、`only-newline.txt`、`crlf.txt`、`utf8-bom.txt` |
| 应当被忽略 | `logs/`、`*.log`、`tmp/`、`*.tmp`（规则见 `.snapignore`） |
| 默认忽略 | `__pycache__/` |
| 由你决定 | `.env`（会被跟踪）、`build/`（会被跟踪） |

## 这个目录本身就是一个 snap 仓库

`.snap/` 是它自己的版本库（和上层项目的仓库相互独立，互不影响）。可以直接在这里试：

```bash
snap -t              # 看改了什么
snap -s "说明"        # 存一个版本
snap -l              # 看历史
snap -c <版本前缀>    # 切回某个版本
```

## iso/ —— 性能测试用的大文件

`iso/` 里是 6 个系统镜像（共 15GB），专门用来测性能和边界：
大文件流式处理、多线程分块压缩、磁盘吞吐都是拿它们量的。

**注意**：因为这里有 15GB 数据，`scenario.sh` 的保护机制会**拒绝重建本目录**
（它的保护条件是"目录里有非脚本创建的内容"）。想跑场景测试请另指一个临时沙箱：

```bash
SNAP_SANDBOX=/tmp/snap-sandbox bash ../rust/snap/tests/scenario.sh
```

## 重跑测试

**场景测试**（109 项检查，会把本目录整个重建后从头跑一遍；需要 git-bash）：

```bash
SNAP_SANDBOX=/tmp/snap-sandbox bash ../rust/snap/tests/scenario.sh
```

**自动化回归测试**（8 项单元测试 + 30 项命令行集成测试）：

```bash
cd ../rust/snap
cargo test
```

两者的区别：`cargo test` 是常驻的回归测试，用临时目录、跑得快；`scenario.sh`
在这个沙箱里按真实使用顺序走一遍，覆盖更宽，也方便人工翻看每一步结果。
