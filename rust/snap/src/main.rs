//! snap - 极简项目版本快照工具（Rust 版）
//!
//! 用法（短写法、首字母、完整写法三者等价）:
//!     snap -i / snap i / snap init              初始化当前目录
//!     snap -t / snap t / snap status            查看工作区状态
//!     snap -s "说明" / snap save "说明"           保存当前状态为新版本
//!     snap -l / snap l / snap list              列出所有版本
//!     snap -v <版本> / snap show <版本>           查看版本详情
//!     snap -c <版本> / snap checkout <版本>       切换到指定版本
//!     snap -d <版本> [版本2] / snap diff ...      比较差异
//!     snap -g / snap g / snap gc                清理未被引用的对象
//!
//! 数据格式与 Python 版一致，两个实现可以读写同一个仓库。

mod chunk;
mod docs;
mod ignore;
mod store;
mod winout;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;

use store::{DATA_DIR, Repo, Version};

// ---------------------------------------------------------------- 错误

#[derive(Debug)]
pub struct SnapError(String);

impl SnapError {
    pub fn new(msg: impl Into<String>) -> Self {
        SnapError(msg.into())
    }
}

impl std::fmt::Display for SnapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<io::Error> for SnapError {
    fn from(e: io::Error) -> Self {
        SnapError(format!("文件操作失败: {e}"))
    }
}

type Result<T> = std::result::Result<T, SnapError>;

// ---------------------------------------------------------------- 入口

fn prog_name() -> String {
    let raw = std::env::args().next().unwrap_or_default();
    let stem = PathBuf::from(&raw)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if stem.is_empty()
        || matches!(
            stem.to_lowercase().as_str(),
            "python" | "pythonw" | "py" | "python3"
        )
    {
        "snap".to_string()
    } else {
        stem
    }
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let prog = prog_name();
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let argv = expand_short_flag(&raw);

    if argv.is_empty() {
        // 只敲了 snap 没带子命令: 直接给用法
        winout::out(&docs::render_help(&prog));
        winout::out("\n");
        return 1;
    }
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        winout::out(&docs::render_help(&prog));
        winout::out("\n");
        return 0;
    }

    let Some(cmd) = docs::find_cmd(&argv[0]) else {
        return usage_error(
            &prog,
            &format!("argument cmd: invalid choice: '{}'", argv[0]),
        );
    };

    let rest = &argv[1..];
    let mut message = String::new();
    let mut positional: Vec<String> = Vec::new();

    match cmd.name {
        "save" => {
            let mut i = 0;
            while i < rest.len() {
                let a = &rest[i];
                if a == "-m" || a == "--message" {
                    i += 1;
                    match rest.get(i) {
                        Some(v) => message = v.clone(),
                        None => {
                            return usage_error(
                                &prog,
                                "argument -m/--message: expected one argument",
                            );
                        }
                    }
                } else if let Some(v) = a.strip_prefix("--message=") {
                    message = v.to_string();
                } else if let Some(v) = a.strip_prefix("-m=") {
                    message = v.to_string();
                } else if a.starts_with('-') {
                    return usage_error(
                        &prog,
                        &format!("unrecognized arguments: {a}（说明以 - 开头时写成 -m=\"...\"）"),
                    );
                } else if positional.is_empty() {
                    positional.push(a.clone());
                } else {
                    return usage_error(&prog, &format!("unrecognized arguments: {a}"));
                }
                i += 1;
            }
            if message.is_empty() {
                message = positional.first().cloned().unwrap_or_default();
            }
        }
        "show" | "checkout" => {
            for a in rest {
                if a.starts_with('-') {
                    return usage_error(&prog, &format!("unrecognized arguments: {a}"));
                }
                positional.push(a.clone());
            }
            if positional.is_empty() {
                return usage_error(&prog, "the following arguments are required: version");
            }
            if positional.len() > 1 {
                return usage_error(&prog, &format!("unrecognized arguments: {}", positional[1]));
            }
        }
        "diff" => {
            for a in rest {
                if a.starts_with('-') {
                    return usage_error(&prog, &format!("unrecognized arguments: {a}"));
                }
                positional.push(a.clone());
            }
            if positional.is_empty() {
                return usage_error(&prog, "the following arguments are required: v1");
            }
            if positional.len() > 2 {
                return usage_error(&prog, &format!("unrecognized arguments: {}", positional[2]));
            }
        }
        _ => {
            if let Some(a) = rest.iter().find(|a| a.starts_with('-')) {
                return usage_error(&prog, &format!("unrecognized arguments: {a}"));
            }
            if let Some(a) = rest.first() {
                return usage_error(&prog, &format!("unrecognized arguments: {a}"));
            }
        }
    }

    let repo = Repo::current();
    if cmd.needs_repo && !repo.is_initialized() {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        return fail(&format!("未初始化，请先运行: {prog} -i\n(当前目录: {cwd})"));
    }

    let r = match cmd.name {
        "init" => cmd_init(),
        "status" => cmd_status(&repo),
        "save" => cmd_save(&repo, &message),
        "list" => cmd_list(&repo),
        "show" => cmd_show(&repo, &positional[0]),
        "checkout" => cmd_checkout(&repo, &positional[0]),
        "diff" => cmd_diff(&repo, &positional[0], positional.get(1).map(|s| s.as_str())),
        "gc" => cmd_gc(&repo),
        _ => unreachable!(),
    };

    match r {
        Ok(()) => 0,
        Err(e) => fail(&e.to_string()),
    }
}

fn fail(msg: &str) -> i32 {
    errln!("错误: {msg}");
    1
}

fn usage_error(prog: &str, msg: &str) -> i32 {
    errln!("{}", docs::usage_line(prog));
    errln!("{prog}: error: {msg}");
    2
}

/// snap -s "说明" → snap save "说明"
fn expand_short_flag(argv: &[String]) -> Vec<String> {
    match argv.first().and_then(|a| docs::short_flag_to_cmd(a)) {
        Some(name) => {
            let mut out = vec![name.to_string()];
            out.extend(argv[1..].iter().cloned());
            out
        }
        None => argv.to_vec(),
    }
}

// ---------------------------------------------------------------- 时间

fn now_secs() -> i64 {
    chrono::Local::now().timestamp()
}

fn fmt_time(ts: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => ts.to_string(),
    }
}

// ---------------------------------------------------------------- 命令

enum DocAction {
    None,
    Created,
    Refreshed,
}

/// 在数据目录里放一份命令对照表；已存在就保留，除非它是旧版生成的（内容里还写着 vsnap）。
fn write_help_doc(repo: &Repo, refresh_stale: bool) -> Result<DocAction> {
    let path = repo.data_path(&[store::HELP_DOC_NAME]);
    let action = if path.is_file() {
        if !refresh_stale {
            return Ok(DocAction::None);
        }
        let old = fs::read_to_string(&path).unwrap_or_default();
        if !old.contains("vsnap") {
            return Ok(DocAction::None);
        }
        DocAction::Refreshed
    } else {
        DocAction::Created
    };
    fs::write(&path, docs::render_help_doc(&prog_name()))?;
    Ok(action)
}

fn cmd_init() -> Result<()> {
    let cwd = std::env::current_dir()?;

    // 当前目录自己已经是仓库：只做旧目录改名和对照表补齐
    if let Some(name) = store::data_dir_here(&cwd) {
        let dir = Repo {
            root: cwd.clone(),
            dir: name,
        }
        .migrate_legacy_dir();
        let repo = Repo {
            root: cwd.clone(),
            dir,
        };
        outln!("已经初始化过了: {}", repo.data_path(&[]).display());
        let doc = repo.data_path(&[store::HELP_DOC_NAME]);
        match write_help_doc(&repo, true)? {
            DocAction::Created => outln!("已补齐命令对照表: {}", doc.display()),
            DocAction::Refreshed => outln!("已更新命令对照表: {}", doc.display()),
            DocAction::None => {}
        }
        return Ok(());
    }

    // 上层若已有仓库，这里会成为一个独立的嵌套仓库，先告知一声
    let outer = Repo::discover(&cwd);

    fs::create_dir_all(cwd.join(DATA_DIR).join("objects"))?;
    fs::create_dir_all(cwd.join(DATA_DIR).join("versions"))?;
    let repo = Repo {
        root: cwd.clone(),
        dir: DATA_DIR.to_string(),
    };
    repo.write_head(None)?;
    write_help_doc(&repo, false)?;
    outln!("初始化完成: {}", repo.data_path(&[]).display());
    outln!(
        "命令对照表: {}",
        PathBuf::from(DATA_DIR).join(store::HELP_DOC_NAME).display()
    );
    outln!("提示: 可在项目根目录创建 .snapignore 排除文件");
    if let Some(o) = outer {
        outln!(
            "注意: 上层已有一个仓库 {}，当前目录成为独立的嵌套仓库；",
            o.root.display()
        );
        outln!("      在本目录及其子目录里执行命令都会用这一个。");
    }
    Ok(())
}

fn cmd_save(repo: &Repo, message: &str) -> Result<()> {
    let tree = repo.scan_tree()?;
    let head = repo.read_head()?;

    if let Some(h) = &head
        && repo.load_version(h)?.tree == tree
    {
        outln!("工作区与当前版本一致，无需保存");
        return Ok(());
    }

    let parents: Vec<String> = head.iter().cloned().collect();
    let vid = store::version_id(&parents, &tree, message);

    if repo.version_path(&vid).exists() {
        // 这个状态以前就存过：保留它原来的时间和序号，只把 HEAD 移过去
        repo.write_head(Some(&vid))?;
        outln!("该状态此前已保存过 {vid}，已把 HEAD 移过去  {message}");
        return Ok(());
    }

    let n = tree.len();
    let v = Version {
        id: vid.clone(),
        parents,
        tree,
        message: message.to_string(),
        time: now_secs(),
        seq: repo.next_seq(),
    };
    fs::write(repo.version_path(&vid), store::version_json(&v))?;
    repo.write_head(Some(&vid))?;
    let mut index = store::Index::load(repo);
    index.reset_to(repo, &v.tree);
    let _ = index.save(repo);
    outln!("已保存 {vid}  ({n} 个文件)  {message}");
    Ok(())
}

fn cmd_list(repo: &Repo) -> Result<()> {
    let versions = repo.all_versions();
    if versions.is_empty() {
        outln!("(还没有任何版本)");
        return Ok(());
    }
    let head = repo.read_head()?;
    for v in versions.iter().rev() {
        let mark = if head.as_deref() == Some(v.id.as_str()) {
            "*"
        } else {
            " "
        };
        let parents = if v.parents.is_empty() {
            "-".to_string()
        } else {
            v.parents
                .iter()
                .map(|p| p.chars().take(6).collect::<String>())
                .collect::<Vec<_>>()
                .join(",")
        };
        outln!(
            "{mark} {}  {}  <-{}  {}",
            v.id,
            fmt_time(v.time),
            parents,
            v.message
        );
    }
    Ok(())
}

fn cmd_show(repo: &Repo, prefix: &str) -> Result<()> {
    let id = repo.resolve(prefix)?;
    let v = repo.load_version(&id)?;
    outln!("版本:   {}", v.id);
    outln!("时间:   {}", fmt_time(v.time));
    outln!(
        "父版本: {}",
        if v.parents.is_empty() {
            "-".to_string()
        } else {
            v.parents.join(", ")
        }
    );
    outln!("说明:   {}", v.message);
    outln!("文件 ({}):", v.tree.len());
    for p in v.tree.keys() {
        outln!("  {p}");
    }
    Ok(())
}

fn cmd_status(repo: &Repo) -> Result<()> {
    let head = repo.read_head()?;
    let base: BTreeMap<String, String> = match &head {
        Some(h) => {
            let v = repo.load_version(h)?;
            outln!("当前版本: {}  {}", v.id, v.message);
            v.tree
        }
        None => {
            outln!("当前版本: (无)");
            BTreeMap::new()
        }
    };

    let cur = repo.workspace_hashes()?;
    let added: Vec<&String> = cur.keys().filter(|k| !base.contains_key(*k)).collect();
    let removed: Vec<&String> = base.keys().filter(|k| !cur.contains_key(*k)).collect();
    let modified: Vec<&String> = cur
        .keys()
        .filter(|k| base.get(*k).is_some_and(|h| *h != cur[*k]))
        .collect();

    if added.is_empty() && removed.is_empty() && modified.is_empty() {
        outln!("工作区干净");
        return Ok(());
    }
    if !added.is_empty() {
        outln!("新增:");
        for p in &added {
            outln!("  + {p}");
        }
    }
    if !modified.is_empty() {
        outln!("修改:");
        for p in &modified {
            outln!("  M {p}");
        }
    }
    if !removed.is_empty() {
        outln!("删除:");
        for p in &removed {
            outln!("  - {p}");
        }
    }
    Ok(())
}

fn cmd_diff(repo: &Repo, v1: &str, v2: Option<&str>) -> Result<()> {
    let id1 = repo.resolve(v1)?;
    let t1 = repo.load_version(&id1)?.tree;
    let (t2, label2) = match v2 {
        Some(v) => {
            let id2 = repo.resolve(v)?;
            (repo.load_version(&id2)?.tree, id2)
        }
        None => (repo.workspace_hashes()?, "工作区".to_string()),
    };

    let added: Vec<&String> = t2.keys().filter(|k| !t1.contains_key(*k)).collect();
    let removed: Vec<&String> = t1.keys().filter(|k| !t2.contains_key(*k)).collect();
    let modified: Vec<&String> = t1
        .keys()
        .filter(|k| t2.get(*k).is_some_and(|h| *h != t1[*k]))
        .collect();

    outln!("--- {id1}");
    outln!("+++ {label2}");
    for p in &added {
        outln!("  + {p}");
    }
    for p in &modified {
        outln!("  M {p}");
    }
    for p in &removed {
        outln!("  - {p}");
    }
    if added.is_empty() && removed.is_empty() && modified.is_empty() {
        outln!("  (无差异)");
    }
    Ok(())
}

fn cmd_gc(repo: &Repo) -> Result<()> {
    let mut referenced: HashSet<String> = HashSet::new();
    for v in repo.all_versions() {
        referenced.extend(v.tree.values().cloned());
    }

    let obj_root = repo.data_path(&["objects"]);
    if !obj_root.is_dir() {
        outln!("对象库为空");
        return Ok(());
    }

    let mut removed = 0u64;
    let mut freed = 0u64;
    for d in fs::read_dir(&obj_root)?.flatten() {
        let dpath = d.path();
        if !dpath.is_dir() {
            continue;
        }
        let prefix = d.file_name().to_string_lossy().to_string();
        for f in fs::read_dir(&dpath)?.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            if referenced.contains(&format!("{prefix}{name}")) {
                continue;
            }
            if let Ok(md) = f.metadata() {
                freed += md.len();
            }
            if fs::remove_file(f.path()).is_ok() {
                removed += 1;
            }
        }
        let _ = fs::remove_dir(&dpath); // 空了就删
    }
    // 顺手清掉中断留下的临时对象
    for f in fs::read_dir(&obj_root)?.flatten() {
        let name = f.file_name().to_string_lossy().to_string();
        if name.starts_with(".tmp-") {
            if let Ok(md) = f.metadata() {
                freed += md.len();
            }
            if fs::remove_file(f.path()).is_ok() {
                removed += 1;
            }
        }
    }
    outln!("清理了 {removed} 个对象，释放 {freed} 字节");
    Ok(())
}

// -------------------------------------------------------- checkout

struct Plan {
    to_delete: Vec<String>,
    to_write: Vec<String>,
    lost: Vec<String>,
}

/// 计划：要删哪些、要写哪些、哪些本地内容会丢（覆盖/删除后无法从现有版本找回）。
fn checkout_plan(
    repo: &Repo,
    target: &BTreeMap<String, String>,
    old_tree: &BTreeMap<String, String>,
    index: &mut store::Index,
) -> Result<Plan> {
    let mut to_delete: Vec<String> = old_tree
        .keys()
        .filter(|k| !target.contains_key(*k))
        .cloned()
        .collect();
    to_delete.sort();

    let mut to_write = Vec::new();
    let mut lost = Vec::new();

    for (rel, h) in target.iter() {
        store::check_rel(rel)?;
        let full = repo.work_path(rel);
        if full.is_file() {
            let cur = index.hash_of(repo, rel)?; // 没变过就直接用索引里的哈希
            if &cur == h {
                continue; // 内容一样，不用写
            }
            to_write.push(rel.clone());
            if old_tree.get(rel) != Some(&cur) {
                lost.push(rel.clone());
            }
        } else {
            to_write.push(rel.clone());
        }
    }

    for rel in &to_delete {
        store::check_rel(rel)?;
        let full = repo.work_path(rel);
        if full.is_file() {
            let cur = index.hash_of(repo, rel)?;
            if old_tree.get(rel) != Some(&cur) {
                lost.push(rel.clone());
            }
        }
    }

    Ok(Plan {
        to_delete,
        to_write,
        lost,
    })
}

/// path 会不会被本次删除阶段清掉（版本里正在删的文件，或里面只剩这些文件的目录）。
fn clearable(repo: &Repo, path: &std::path::Path, to_delete: &HashSet<&String>) -> bool {
    if path.is_file() {
        return store::rel_posix(&repo.root, path).is_some_and(|rel| to_delete.contains(&rel));
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            return false;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(rel) = store::rel_posix(&repo.root, &p) {
                if !to_delete.contains(&rel) {
                    return false;
                }
            } else {
                return false;
            }
        }
    }
    true
}

/// 动任何文件之前，把注定会失败的情况一次性找出来。
fn preflight(repo: &Repo, plan: &Plan, target: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut problems = Vec::new();
    let dels: HashSet<&String> = plan.to_delete.iter().collect();

    for rel in &plan.to_write {
        let h = &target[rel];
        if !repo.object_exists(h) {
            problems.push((rel.clone(), format!("对象库缺少对象 {}…", &h[..8])));
            continue;
        }
        let full = repo.work_path(rel);
        if full.is_dir() && !clearable(repo, &full, &dels) {
            problems.push((
                rel.clone(),
                "同名目录里有未保存的内容，需要先把它移走".to_string(),
            ));
            continue;
        }
        let mut blocked: Option<String> = None;
        let mut d = full.parent().map(|p| p.to_path_buf());
        while let Some(cur) = d {
            if cur == repo.root {
                break;
            }
            if cur.is_file() && !clearable(repo, &cur, &dels) {
                blocked = store::rel_posix(&repo.root, &cur);
                break;
            }
            d = cur.parent().map(|p| p.to_path_buf());
        }
        if let Some(b) = blocked {
            problems.push((
                rel.clone(),
                format!("上层路径 {b} 是个文件，需要先把它移走"),
            ));
            continue;
        }
        if full.is_file() && is_readonly(&full) {
            problems.push((rel.clone(), "文件只读，没有写权限".to_string()));
        }
    }

    for rel in &plan.to_delete {
        let full = repo.work_path(rel);
        if full.is_dir() && !clearable(repo, &full, &dels) {
            problems.push((
                rel.clone(),
                "同名目录里有未保存的内容，需要先把它移走".to_string(),
            ));
        }
    }
    problems
}

fn is_readonly(path: &std::path::Path) -> bool {
    fs::metadata(path)
        .map(|md| md.permissions().readonly())
        .unwrap_or(false)
}

/// 读一次确认；stdin 不可用（管道/CI）时按取消处理，绝不猜。
fn confirm(prompt: &str) -> Result<()> {
    winout::out(prompt);
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => {
            winout::out("\n");
            Err(SnapError::new(
                "无法读取确认输入（stdin 不可用），已取消，工作区未做任何改动",
            ))
        }
        Ok(_) => {
            let a = line.trim().to_lowercase();
            if a == "y" || a == "yes" {
                Ok(())
            } else {
                Err(SnapError::new("已取消，工作区未做任何改动"))
            }
        }
    }
}

fn cmd_checkout(repo: &Repo, prefix: &str) -> Result<()> {
    let vid = repo.resolve(prefix)?;
    let target = repo.load_version(&vid)?.tree;
    let old_tree = match repo.read_head()? {
        Some(h) => repo.load_version(&h)?.tree,
        None => BTreeMap::new(),
    };

    let mut index = store::Index::load(repo);
    let plan = checkout_plan(repo, &target, &old_tree, &mut index)?;

    // 1) 预检：有问题就一个文件都不动
    let problems = preflight(repo, &plan, &target);
    if !problems.is_empty() {
        outln!("无法切换到 {vid}，以下问题需要先处理:");
        for (rel, why) in &problems {
            outln!("  ! {rel}: {why}");
        }
        return Err(SnapError::new("checkout 已中止，工作区未做任何改动"));
    }

    // 2) 安全确认：会丢内容时列出来
    if !plan.lost.is_empty() {
        outln!(
            "警告: 以下 {} 个文件在磁盘上的内容未保存到任何版本，",
            plan.lost.len()
        );
        outln!("checkout 会覆盖或删除它们，之后无法找回:");
        for rel in plan.lost.iter().take(20) {
            outln!("    {rel}");
        }
        if plan.lost.len() > 20 {
            outln!("    ... 另外 {} 个", plan.lost.len() - 20);
        }
        confirm("继续? [y/N] ")?;
    }

    // 3) 执行
    let mut fails: Vec<(String, String)> = Vec::new();

    for rel in &plan.to_delete {
        let full = repo.work_path(rel);
        match fs::remove_file(&full) {
            Ok(()) => outln!("删除 {rel}"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                fails.push((rel.clone(), e.to_string()));
                continue;
            }
        }
        // 清理随之变空的目录
        let mut d = full.parent().map(|p| p.to_path_buf());
        while let Some(cur) = d {
            if cur == repo.root {
                break;
            }
            if fs::remove_dir(&cur).is_err() {
                break;
            }
            d = cur.parent().map(|p| p.to_path_buf());
        }
    }

    for rel in &plan.to_write {
        let full = repo.work_path(rel);
        let done = (|| -> Result<()> {
            if full.is_dir() {
                fs::remove_dir(&full)?; // 空目录可以直接换成文件
            }
            if let Some(dir) = full.parent() {
                fs::create_dir_all(dir)?;
            }
            // 流式解压写入：大文件也不会把内存吃满
            repo.checkout_object_to(&target[rel], &full)?;
            Ok(())
        })();
        match done {
            Ok(()) => outln!("写入 {rel}"),
            Err(e) => fails.push((rel.clone(), e.to_string())),
        }
    }

    // 4) 只有全部成功才移动 HEAD
    if !fails.is_empty() {
        errln!("以下文件处理失败:");
        for (rel, why) in &fails {
            errln!("  ! {rel}: {why}");
        }
        return Err(SnapError::new(format!(
            "切换到 {vid} 未完成，工作区处于部分完成状态，请处理上述问题后重新运行"
        )));
    }

    repo.write_head(Some(&vid))?;
    // 工作区现在就是目标版本这棵树，直接刷新索引，下次不用重新哈希
    index.reset_to(repo, &target);
    let _ = index.save(repo);
    outln!("已切换到 {vid}");
    Ok(())
}
