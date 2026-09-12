//! 命令清单：解析器、帮助文本、以及 init 时写进数据目录的命令对照表都从这里生成。

pub struct Command {
    pub name: &'static str,
    pub alias: &'static str,
    pub flag: &'static str,
    pub desc: &'static str,
    pub hint: &'static str, // 后面跟的参数提示，空串表示没有
    pub needs_repo: bool,
}

pub const COMMANDS: [Command; 8] = [
    Command {
        name: "init",
        alias: "i",
        flag: "-i",
        desc: "初始化当前目录",
        hint: "",
        needs_repo: false,
    },
    Command {
        name: "status",
        alias: "t",
        flag: "-t",
        desc: "查看工作区状态",
        hint: "",
        needs_repo: true,
    },
    Command {
        name: "save",
        alias: "s",
        flag: "-s",
        desc: "保存当前状态为新版本",
        hint: "\"说明\"",
        needs_repo: true,
    },
    Command {
        name: "list",
        alias: "l",
        flag: "-l",
        desc: "列出所有版本",
        hint: "",
        needs_repo: true,
    },
    Command {
        name: "show",
        alias: "v",
        flag: "-v",
        desc: "查看版本详情",
        hint: "<版本>",
        needs_repo: true,
    },
    Command {
        name: "checkout",
        alias: "c",
        flag: "-c",
        desc: "切换到指定版本",
        hint: "<版本>",
        needs_repo: true,
    },
    Command {
        name: "diff",
        alias: "d",
        flag: "-d",
        desc: "比较差异",
        hint: "<版本> [版本2]",
        needs_repo: true,
    },
    Command {
        name: "gc",
        alias: "g",
        flag: "-g",
        desc: "清理未被引用的对象",
        hint: "",
        needs_repo: true,
    },
];

/// -x 短写法 → 子命令名。
pub fn short_flag_to_cmd(arg: &str) -> Option<&'static str> {
    COMMANDS.iter().find(|c| c.flag == arg).map(|c| c.name)
}

/// 子命令名或首字母 → 命令。
pub fn find_cmd(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name || c.alias == name)
}

const HELP_DOC: &str = "\
# {p} 命令对照表

`{p}` 是下面命令的调用名，也就是你运行的那个程序（`snap` 或 `snap.exe`）。
三种写法完全等价。

| 想做的事 | 短写法 | 首字母 | 完整写法 |
| --- | --- | --- | --- |
{table}

版本ID可以只写前几位前缀，只要唯一即可（例如 `{p} -c a55b`）。

## 典型流程

    {p} -i                    只做一次，在当前目录建版本库
    {p} -t                    看改了什么：+ 新增 / M 修改 / - 删除
    {p} -s \"完成登录功能\"       把当前状态存成一个版本
    {p} -l                    看历史，带 * 的是当前所在版本
    {p} -d a55b               某个版本 vs 现在的工作区
    {p} -d a55b c0ec          两个版本对比
    {p} -c a55b               需要时切回那个版本
    {p} -g                    清理没被引用的残留对象

命令在项目的任意子目录里都能用，会自动向上找到项目根的 .snap。

## 切换版本时的安全提示

工作区里有没保存到任何版本的内容时，`-c` 会先列出来并询问 `继续? [y/N]`，
直接回车或答 n 就是取消，一个文件都不会动。执行前还会做一次预检（对象库
缺对象、目标路径被同名文件占住、文件只读等），有问题就一个文件都不改并报错
退出；只有全部成功才会更新当前版本。

## .snapignore 忽略规则（放在项目根目录）

    *.log          不含 \"/\" 的模式，匹配任意一层里的同名条目
    logs/          以 \"/\" 结尾表示目录规则，匹配该目录及其下所有内容
    keep/c.txt     含 \"/\" 的模式相对项目根目录定位，\"*\" 不跨目录层级
    src/**/*.py    \"**\" 可跨任意层级
    # 开头的行和空行忽略；不支持 \"!\" 反选

默认已经忽略：`.snap/` `.git/` `__pycache__/` `*.pyc` `*.pyo` `.DS_Store`
`*.swp` `*~`

## 其他要点

- 本程序自己的文件不会进版本库，放在项目里也不会把工具自己存进去。
- 相同内容只存一份；没改动的文件不会重复占用空间。
- 内容没变时 `-s` 不会产生新版本，也不会重复记录时间。
- 需要排除密钥、日志等文件，用 `.snapignore`（不支持 `!` 反选）。
- 实时帮助：`{p} -h` 或 `{p} --help`
";

/// 生成写进数据目录的命令对照表。
pub fn render_help_doc(prog: &str) -> String {
    let mut rows = Vec::new();
    for c in COMMANDS.iter() {
        let hint = if c.hint.is_empty() {
            String::new()
        } else {
            format!(" {}", c.hint)
        };
        rows.push(format!(
            "| {} | `{p} {f}{h}` | `{p} {a}{h}` | `{p} {n}{h}` |",
            c.desc,
            p = prog,
            f = c.flag,
            a = c.alias,
            n = c.name,
            h = hint
        ));
    }
    HELP_DOC
        .replace("{p}", prog)
        .replace("{table}", &rows.join("\n"))
}

/// --help 末尾的示例段落。
pub fn render_examples(prog: &str) -> String {
    let mut lines = vec!["常用:".to_string()];
    for c in COMMANDS.iter() {
        let hint = if c.hint.is_empty() {
            String::new()
        } else {
            format!(" {}", c.hint)
        };
        let left = format!("{prog} {}{hint}", c.flag);
        // Python 用 :24 按字符补齐
        let pad = 24usize.saturating_sub(left.chars().count());
        lines.push(format!("  {left}{} {}", " ".repeat(pad), c.desc));
    }
    lines.push(String::new());
    lines.push(
        "子命令也能只写首字母（snap i / snap s \"说明\" ...），完整写法把 -x 换成".to_string(),
    );
    lines.push("子命令名即可（snap -c 相当于 snap checkout）。".to_string());
    lines.push(format!(
        "完整对照表: {prog} -h，或在项目里的 .snap/命令对照表.md"
    ));
    lines.push(format!(
        "{prog} 管理的是运行命令时所在的目录，所以要先 cd 进项目再执行。"
    ));
    lines.join("\n")
}

/// 完整帮助（与 argparse 的输出结构一致）。
pub fn render_help(prog: &str) -> String {
    let mut s = String::new();
    let names: Vec<String> = COMMANDS
        .iter()
        .flat_map(|c| [c.name.to_string(), c.alias.to_string()])
        .collect();
    let list = names.join(",");
    s.push_str(&format!("usage: {prog} [-h] {{{list}}} ...\n\n"));
    s.push_str("极简项目版本快照工具\n\n");
    s.push_str("positional arguments:\n");
    s.push_str(&format!("  {{{list}}}\n"));
    for c in COMMANDS.iter() {
        // 与 argparse 的排版一致：表头补到 18 列，后面再空 2 格
        s.push_str(&format!(
            "    {:<18}  {}\n",
            format!("{} ({})", c.name, c.alias),
            c.desc
        ));
    }
    s.push_str("\noptions:\n");
    s.push_str("  -h, --help            show this help message and exit\n\n");
    s.push_str(&render_examples(prog));
    s
}

/// 用法错误的简短提示（退出码 2）。
pub fn usage_line(prog: &str) -> String {
    let names: Vec<String> = COMMANDS
        .iter()
        .flat_map(|c| [c.name.to_string(), c.alias.to_string()])
        .collect();
    format!("usage: {prog} [-h] {{{}}} ...", names.join(","))
}
