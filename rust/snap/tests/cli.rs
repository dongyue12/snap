//! 命令行集成测试：对编译出来的二进制做黑盒测试。
//!
//! 每个测试在自己独立的临时目录里跑，互不干扰。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_snap");

// ---------------------------------------------------------------- 测试脚手架

/// 独立临时目录，测试结束自动清理。
struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let dir = std::env::temp_dir().join(format!("snap-cli-{}-{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("创建临时目录");
        Tmp(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// 写入一个文件（自动建父目录）。
    fn file(&self, rel: &str, data: &[u8]) {
        let p = self.0.join(rel);
        if let Some(d) = p.parent() {
            fs::create_dir_all(d).expect("创建父目录");
        }
        fs::write(&p, data).expect("写入测试文件");
    }

    fn text(&self, rel: &str, s: &str) {
        self.file(rel, s.as_bytes());
    }

    fn read(&self, rel: &str) -> Vec<u8> {
        fs::read(self.0.join(rel)).expect("读取文件")
    }

    fn exists(&self, rel: &str) -> bool {
        self.0.join(rel).exists()
    }

    fn entries(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.0)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        v.sort();
        v
    }

    fn head(&self) -> Option<String> {
        let s = fs::read_to_string(self.0.join(".snap/HEAD")).ok()?;
        let s = s.trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    fn version(&self, id: &str) -> serde_json::Value {
        let s = fs::read_to_string(self.0.join(".snap/versions").join(format!("{id}.json")))
            .expect("读取版本文件");
        serde_json::from_str(&s).expect("解析版本 JSON")
    }

    fn tree(&self, id: &str) -> Vec<String> {
        let v = self.version(id);
        let mut keys: Vec<String> = v["tree"].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Out {
    code: i32,
    out: String,
    err: String,
}

impl Out {
    fn all(&self) -> String {
        format!("{}{}", self.out, self.err)
    }
    fn ids(&self) -> Vec<String> {
        self.out
            .split_whitespace()
            .filter(|w| w.len() == 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|s| s.to_string())
            .collect()
    }
}

fn convert(o: Output) -> Out {
    Out {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).to_string(),
        err: String::from_utf8_lossy(&o.stderr).to_string(),
    }
}

/// 跑一条命令，stdin 关闭（模拟 CI/管道）。
fn run(dir: &Path, args: &[&str]) -> Out {
    convert(
        Command::new(BIN)
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .expect("执行 snap"),
    )
}

/// 跑一条命令，带环境变量并喂入 stdin。
fn run_with_env(dir: &Path, args: &[&str], env: &[(&str, &str)], input: &str) -> Out {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("启动 snap");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("写入 stdin");
    convert(child.wait_with_output().expect("等待 snap"))
}

/// 跑一条命令并喂入 stdin（用于 checkout 的确认）。
fn run_with_stdin(dir: &Path, args: &[&str], input: &str) -> Out {
    let mut child = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 snap");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("写入 stdin");
    convert(child.wait_with_output().expect("等待 snap"))
}

/// 建一个干净的初始仓库，返回目录。
fn repo(tag: &str) -> Tmp {
    let t = Tmp::new(tag);
    let o = run(t.path(), &["-i"]);
    assert_eq!(o.code, 0, "init 失败: {}", o.all());
    t
}

fn save(dir: &Path, msg: &str) -> String {
    let o = run(dir, &["-s", msg]);
    assert_eq!(o.code, 0, "save 失败: {}", o.all());
    o.ids().first().cloned().unwrap_or_default()
}

// ---------------------------------------------------------------- 测试

#[test]
fn init_creates_repo_and_cheat_sheet() {
    let t = repo("init");
    assert!(t.exists(".snap/HEAD"));
    assert!(t.exists(".snap/versions"));
    assert!(t.exists(".snap/objects"));
    let doc = fs::read_to_string(t.path().join(".snap/命令对照表.md")).expect("命令对照表");
    assert!(doc.starts_with("# snap 命令对照表"), "{doc}");
    assert!(doc.contains("`snap -i`") && doc.contains("`snap i`") && doc.contains("`snap init`"));
    assert!(doc.contains(".snapignore"), "对照表应说明忽略规则");

    // 重复 init 不报错，也不重建
    let again = run(t.path(), &["-i"]);
    assert_eq!(again.code, 0);
    assert!(again.out.contains("已经初始化过了"), "{}", again.out);
}

#[test]
fn needs_repo_before_init() {
    let t = Tmp::new("norepo");
    let o = run(t.path(), &["-t"]);
    assert_ne!(o.code, 0);
    assert!(o.err.contains("未初始化"), "{}", o.err);
}

#[test]
fn status_reports_three_states() {
    let t = repo("status");
    t.text("a.txt", "A");
    t.text("b.txt", "B");
    let v1 = save(t.path(), "v1");

    t.text("a.txt", "A2"); // 修改
    t.text("c.txt", "C"); // 新增
    fs::remove_file(t.path().join("b.txt")).unwrap(); // 删除

    let o = run(t.path(), &["-t"]);
    assert!(o.out.contains("当前版本: "), "{}", o.out);
    assert!(o.out.contains("M a.txt"), "{}", o.out);
    assert!(o.out.contains("+ c.txt"), "{}", o.out);
    assert!(o.out.contains("- b.txt"), "{}", o.out);

    save(t.path(), "v2");
    let clean = run(t.path(), &["-t"]);
    assert!(clean.out.contains("工作区干净"), "{}", clean.out);
    assert_ne!(v1, t.head().unwrap(), "HEAD 应指向新版本");
}

#[test]
fn repeat_save_of_same_state_creates_no_version() {
    let t = repo("dedup");
    t.text("a.txt", "A");
    let v1 = save(t.path(), "v1");
    let o = run(t.path(), &["-s", "又存一次"]);
    assert!(o.out.contains("无需保存"), "{}", o.out);
    assert_eq!(
        fs::read_dir(t.path().join(".snap/versions"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(t.head().unwrap(), v1);
}

#[test]
fn version_id_is_deterministic() {
    // 相同内容 + 相同说明 + 相同父版本 → 同一个版本ID
    let a = repo("det-a");
    let b = repo("det-b");
    for d in [a.path(), b.path()] {
        fs::write(d.join("x.txt"), b"same").unwrap();
    }
    let va = save(a.path(), "第一版");
    let vb = save(b.path(), "第一版");
    assert_eq!(va, vb, "相同状态必须得到同一个版本ID");
    assert_eq!(a.tree(&va), b.tree(&vb));
}

#[test]
fn list_marks_head_and_show_resolves_prefix() {
    let t = repo("list");
    t.text("a.txt", "A");
    let v1 = save(t.path(), "第一版");
    t.text("a.txt", "B");
    let v2 = save(t.path(), "第二版");

    let o = run(t.path(), &["-l"]);
    let lines: Vec<&str> = o.out.lines().collect();
    assert_eq!(lines.len(), 2, "{}", o.out);
    assert!(
        lines[0].starts_with('*') && lines[0].contains(&v2),
        "新的在上面: {}",
        o.out
    );
    assert!(lines[1].contains(&v1), "{}", o.out);

    let s = run(t.path(), &["-v", &v1[..6]]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert!(s.out.contains(&format!("版本:   {v1}")), "{}", s.out);
    assert!(s.out.contains("说明:   第一版"), "{}", s.out);
    assert!(s.out.contains("a.txt"), "{}", s.out);

    let missing = run(t.path(), &["-v", "zzzz"]);
    assert_ne!(missing.code, 0);
    assert!(missing.err.contains("找不到版本"), "{}", missing.err);
    assert!(
        !missing.err.contains("panicked"),
        "不该崩溃: {}",
        missing.err
    );
}

#[test]
fn diff_between_version_and_workspace() {
    let t = repo("diff");
    t.text("a.txt", "A");
    let v1 = save(t.path(), "v1");
    t.text("a.txt", "A2");
    t.text("new.txt", "N");

    let o = run(t.path(), &["-d", &v1[..6]]);
    assert!(
        o.out.contains("M a.txt") && o.out.contains("+ new.txt"),
        "{}",
        o.out
    );

    let same = run(t.path(), &["-d", &v1[..6], &v1[..6]]);
    assert!(same.out.contains("(无差异)"), "{}", same.out);
}

#[test]
fn ignore_rules() {
    let t = repo("ignore");
    t.text("logs/debug.log", "noise");
    t.text("keep/a.log", "keep");
    t.text("top.log", "top");
    t.text("cache", "我是与目录规则同名的普通文件");
    t.text("a.txt", "A");
    t.text(".snapignore", "logs/\ncache/\n*.log\n");

    let v = save(t.path(), "v1");
    let tree = t.tree(&v);
    assert!(
        !tree.iter().any(|k| k.starts_with("logs/")),
        "目录规则 logs/: {tree:?}"
    );
    assert!(
        !tree.contains(&"keep/a.log".to_string()),
        "*.log 应匹配任意层级: {tree:?}"
    );
    assert!(
        !tree.contains(&"top.log".to_string()),
        "*.log 应匹配任意层级: {tree:?}"
    );
    assert!(
        tree.contains(&"cache".to_string()),
        "目录规则不该命中同名普通文件: {tree:?}"
    );
    assert!(tree.contains(&"a.txt".to_string()), "{tree:?}");
    assert!(
        tree.contains(&".snapignore".to_string()),
        "忽略文件本身应被跟踪: {tree:?}"
    );
    assert!(
        !tree.iter().any(|k| k == ".snap" || k.starts_with(".snap/")),
        "数据目录必须被忽略（注意 .snapignore 本身是要跟踪的）: {tree:?}"
    );
}

#[test]
fn ignore_anchoring_and_globstar() {
    let t = repo("anchor");
    t.text("keep/a.log", "n");
    t.text("keep/deep/b.log", "n");
    t.text("other.log", "n");
    t.text(".snapignore", "keep/*.log\n");

    let v = save(t.path(), "v1");
    let tree = t.tree(&v);
    assert!(
        !tree.contains(&"keep/a.log".to_string()),
        "keep/*.log 应命中: {tree:?}"
    );
    assert!(
        tree.contains(&"keep/deep/b.log".to_string()),
        "* 不该跨目录层级: {tree:?}"
    );
    assert!(
        tree.contains(&"other.log".to_string()),
        "含 / 的模式是锚定的: {tree:?}"
    );

    // "**" 可以跨层级
    t.text(".snapignore", "keep/**/*.log\n");
    t.text("keep/a.log", "改了");
    let v2 = save(t.path(), "v2");
    let tree2 = t.tree(&v2);
    assert!(
        !tree2.contains(&"keep/deep/b.log".to_string()),
        "keep/**/*.log 应命中多层: {tree2:?}"
    );
    assert!(
        !tree2.contains(&"keep/a.log".to_string()),
        "**/ 可以是零层: {tree2:?}"
    );
}

#[test]
fn paths_are_posix_and_roundtrip_bytes() {
    let t = repo("paths");
    let bin: Vec<u8> = (0..=255u8).collect();
    t.file("deep/a/b/c.txt", b"deep\n");
    t.file("bin.dat", &bin);
    t.file("empty.txt", b"");
    t.text("中文 文件.txt", "中文内容\n");

    let v = save(t.path(), "v1");
    let tree = t.tree(&v);
    assert!(tree.contains(&"deep/a/b/c.txt".to_string()), "{tree:?}");
    assert!(
        !tree.iter().any(|k| k.contains('\\')),
        "路径键应为正斜杠: {tree:?}"
    );
    assert!(tree.contains(&"中文 文件.txt".to_string()), "{tree:?}");

    // 改乱再切回来
    t.file("bin.dat", b"mutated");
    t.text("deep/a/b/c.txt", "mutated");
    let o = run_with_stdin(t.path(), &["-c", &v[..6]], "y\n");
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(t.read("bin.dat"), bin, "二进制往返必须一致");
    assert_eq!(t.read("empty.txt"), b"", "空文件往返必须一致");
    assert_eq!(t.read("deep/a/b/c.txt"), b"deep\n");
}

#[test]
fn checkout_warns_before_losing_unsaved_content() {
    let t = repo("loss");
    t.text("a.txt", "版本里的内容");
    let v1 = save(t.path(), "v1");

    fs::remove_file(t.path().join("a.txt")).unwrap();
    t.text("a.txt", "用户从未保存的内容");

    // 答 n（回车）→ 一个文件都不动
    let no = run_with_stdin(t.path(), &["-c", &v1[..6]], "\n");
    assert_ne!(no.code, 0);
    assert!(no.out.contains("警告"), "应给出警告: {}", no.out);
    assert!(no.out.contains("a.txt"), "应列出会丢的文件: {}", no.out);
    assert_eq!(
        t.read("a.txt"),
        "用户从未保存的内容".as_bytes(),
        "答 n 时内容必须保留"
    );

    // 没有可读 stdin → 按取消处理
    let eof = run(t.path(), &["-c", &v1[..6]]);
    assert_ne!(eof.code, 0);
    assert_eq!(
        t.read("a.txt"),
        "用户从未保存的内容".as_bytes(),
        "不能擅自覆盖"
    );

    // 答 y → 才覆盖
    let yes = run_with_stdin(t.path(), &["-c", &v1[..6]], "y\n");
    assert_eq!(yes.code, 0, "{}", yes.all());
    assert_eq!(t.read("a.txt"), "版本里的内容".as_bytes());
}

#[test]
fn checkout_preflight_leaves_everything_untouched() {
    let t = repo("preflight");
    t.text("keep.txt", "k");
    t.text("x/inner.txt", "v1 的内容");
    let v1 = save(t.path(), "v1");
    fs::remove_file(t.path().join("x/inner.txt")).unwrap();
    fs::remove_dir(t.path().join("x")).unwrap();
    t.text("y.txt", "v2 的 y");
    let v2 = save(t.path(), "v2");

    // 未跟踪的文件挡住目标版本需要的目录
    t.text("x", "挡路的未跟踪文件");
    let before = t.entries();
    let o = run_with_stdin(t.path(), &["-c", &v1[..6]], "y\n");

    assert_ne!(o.code, 0);
    assert!(!o.err.contains("panicked"), "不该崩溃: {}", o.err);
    assert!(o.all().contains("x"), "应指出挡路的路径: {}", o.all());
    assert!(t.exists("y.txt"), "预检失败不能删掉文件");
    assert_eq!(t.entries(), before, "预检失败时磁盘必须原样");
    assert_eq!(t.head().unwrap(), v2, "预检失败时 HEAD 不能变");
}

#[test]
fn checkout_can_swap_file_and_directory() {
    let t = repo("swap");
    t.text("p", "v1 里 p 是文件");
    let v1 = save(t.path(), "v1 文件p");
    fs::remove_file(t.path().join("p")).unwrap();
    t.text("p/inner.txt", "v2 里 p 是目录");
    save(t.path(), "v2 目录p");

    let o = run_with_stdin(t.path(), &["-c", &v1[..6]], "y\n");
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(t.read("p"), "v1 里 p 是文件".as_bytes());
    assert!(!t.path().join("p").is_dir());
}

#[test]
fn checkout_removes_empty_directories() {
    let t = repo("rmdir");
    t.text("d/sub/x.txt", "x");
    let v1 = save(t.path(), "v1");
    fs::remove_file(t.path().join("d/sub/x.txt")).unwrap();
    save(t.path(), "v2 空目录");

    let o = run_with_stdin(t.path(), &["-c", &v1[..6]], "y\n");
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(t.exists("d/sub/x.txt"), "应重建嵌套文件");

    let back = run_with_stdin(t.path(), &["-c", &t.head().unwrap()[..6]], "y\n");
    assert_eq!(back.code, 0);
    let v2 = fs::read_dir(t.path().join(".snap/versions"))
        .unwrap()
        .count();
    assert_eq!(v2, 2);
}

#[test]
fn read_only_file_is_caught_by_preflight() {
    let t = repo("readonly");
    t.text("a.txt", "v1");
    let v1 = save(t.path(), "v1");
    t.text("a.txt", "v2");
    let v2 = save(t.path(), "v2");

    let f = t.path().join("a.txt");
    let mut perm = fs::metadata(&f).unwrap().permissions();
    perm.set_readonly(true);
    fs::set_permissions(&f, perm).unwrap();

    let o = run_with_stdin(t.path(), &["-c", &v1[..6]], "y\n");

    // 恢复可写，避免影响清理
    let mut perm = fs::metadata(&f).unwrap().permissions();
    perm.set_readonly(false);
    fs::set_permissions(&f, perm).unwrap();

    assert_ne!(o.code, 0);
    assert!(!o.err.contains("panicked"), "不该崩溃: {}", o.err);
    assert!(o.all().contains("只读"), "应提示只读: {}", o.all());
    assert_eq!(t.head().unwrap(), v2, "HEAD 不能变");
}

#[test]
fn short_flags_and_aliases_are_equivalent() {
    let t = repo("short");
    t.text("a.txt", "A");
    let v1 = save(t.path(), "v1");

    // 短写法
    let s = run(t.path(), &["-t"]);
    assert_eq!(s.code, 0);
    assert!(s.out.contains("工作区干净"), "{}", s.out);

    // 首字母
    let a = run(t.path(), &["t"]);
    assert_eq!(a.out, s.out, "首字母与短写法输出必须一致");

    // 带说明的三种写法，指向同一条命令
    t.text("a.txt", "B");
    let one = run(t.path(), &["-s", "改过了"]);
    assert_eq!(one.code, 0, "{}", one.all());

    let sh = run(t.path(), &["-l"]);
    let al = run(t.path(), &["l"]);
    assert_eq!(sh.out, al.out);
    assert!(sh.out.contains(&v1));

    let full = run(t.path(), &["show", &v1[..6]]);
    let short = run(t.path(), &["-v", &v1[..6]]);
    assert_eq!(full.out, short.out);
}

#[test]
fn running_program_excludes_itself() {
    let t = repo("selfexe");
    t.text("keep.txt", "k");
    // 把二进制复制进项目，并用它自己跑
    let me = t.path().join("snap.exe");
    fs::copy(BIN, &me).unwrap();
    let run_me = |args: &[&str]| -> Out {
        convert(
            Command::new(&me)
                .args(args)
                .current_dir(t.path())
                .stdin(Stdio::null())
                .output()
                .expect("执行复制进来的 snap.exe"),
        )
    };
    let o = run_me(&["-s", "v1"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let v = o.ids()[0].clone();
    let tree = t.tree(&v);
    assert!(
        !tree.contains(&"snap.exe".to_string()),
        "程序自己不该进版本: {tree:?}"
    );
    assert!(tree.contains(&"keep.txt".to_string()), "{tree:?}");
}

#[test]
fn gc_removes_only_unreferenced_objects() {
    let t = repo("gc");
    t.text("a.txt", "A");
    save(t.path(), "v1");

    let before = fs::read_dir(t.path().join(".snap/objects"))
        .unwrap()
        .count();
    // 手工放一个没人引用的对象
    let orphan = t.path().join(".snap/objects/ab");
    fs::create_dir_all(&orphan).unwrap();
    fs::write(orphan.join("cdef"), b"not a valid blob").unwrap();

    let o = run(t.path(), &["-g"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.out.contains("清理了 1 个对象"), "{}", o.out);
    let after = fs::read_dir(t.path().join(".snap/objects"))
        .unwrap()
        .count();
    assert!(after <= before, "不应删掉被引用的对象: {before} -> {after}");

    // 版本仍然能完整取回
    let s = run(t.path(), &["-t"]);
    assert!(
        s.out.contains("工作区干净"),
        "gc 后仓库应仍然自洽: {}",
        s.out
    );
}

#[test]
fn legacy_vsnap_repo_is_recognized_and_migrated() {
    let t = repo("legacy");
    t.text("a.txt", "A");
    let v1 = save(t.path(), "v1");

    // 伪造成旧版建的仓库
    fs::rename(t.path().join(".snap"), t.path().join(".vsnap")).unwrap();
    t.text(".vsnapignore", "*.log\n");
    t.text("debug.log", "noise");

    let before = run(t.path(), &["-t"]);
    assert_eq!(before.code, 0, "旧目录应能直接用: {}", before.all());
    assert!(
        !before.out.contains("debug.log"),
        "旧忽略文件仍要生效: {}",
        before.out
    );
    assert!(t.exists(".vsnap"), "没 init 时不该自动改名");

    let init = run(t.path(), &["-i"]);
    assert!(
        init.out.contains("已把数据目录 .vsnap 改名为 .snap"),
        "{}",
        init.out
    );
    assert!(t.exists(".snap") && !t.exists(".vsnap"));

    let after = run(t.path(), &["-l"]);
    assert!(after.out.contains(&v1), "改名后版本不能丢: {}", after.out);
    assert_eq!(t.head().unwrap(), v1);
}

#[test]
fn empty_version_prefix_is_rejected() {
    let t = repo("empty-prefix");
    t.text("a.txt", "A");
    let v1 = save(t.path(), "v1");

    // 空前缀不能被当成"匹配所有版本"，也不能悄悄命中唯一那个版本
    for args in [vec!["-c", ""], vec!["-v", ""], vec!["-d", ""]] {
        let o = run(t.path(), &args);
        assert_ne!(o.code, 0, "空前缀应被拒绝: {}", o.all());
        assert!(o.err.contains("版本ID"), "应提示需要版本ID: {}", o.err);
        assert!(!o.err.contains("panicked"), "不该崩溃: {}", o.err);
    }
    assert_eq!(t.head().unwrap(), v1, "被拒绝的操作不该改动 HEAD");
}

#[test]
fn nested_init_creates_independent_repo() {
    let outer = repo("nested");
    outer.text("a.txt", "A");
    let outer_v = save(outer.path(), "外层版本");

    let inner = outer.path().join("sub");
    fs::create_dir_all(&inner).unwrap();
    fs::write(inner.join("b.txt"), b"B").unwrap();

    let o = run(&inner, &["-i"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.out.contains("嵌套仓库"), "应提示成为嵌套仓库: {}", o.out);
    assert!(inner.join(".snap").is_dir(), "子目录里应建立自己的仓库");

    // 内层命令用内层仓库
    let s = run(&inner, &["-s", "内层版本"]);
    assert_eq!(s.code, 0, "{}", s.all());
    let inner_id = s.ids()[0].clone();
    let st = run(&inner, &["-t"]);
    assert!(st.out.contains("工作区干净"), "{}", st.out);
    let ls = run(&inner, &["-l"]);
    assert!(ls.out.contains(&inner_id), "{}", ls.out);

    // 外层仓库不受影响，且把内层数据目录忽略掉
    assert_eq!(outer.head().unwrap(), outer_v, "外层 HEAD 不该变");
    let outer_status = run(outer.path(), &["-t"]);
    assert!(
        outer_status.out.contains("+ sub/b.txt"),
        "{}",
        outer_status.out
    );
    assert!(
        !outer_status.out.contains("sub/.snap"),
        "内层数据目录应被忽略: {}",
        outer_status.out
    );

    // 外层仓库根目录再 init 仍然是"已经初始化过了"
    let again = run(outer.path(), &["-i"]);
    assert!(again.out.contains("已经初始化过了"), "{}", again.out);
}

#[test]
fn help_and_usage_errors() {
    let t = repo("help");
    let none = run(t.path(), &[]);
    assert_ne!(none.code, 0, "没带子命令应给出用法");
    assert!(none.out.contains("usage:"), "{}", none.out);
    assert!(
        none.out.contains("snap -s"),
        "示例里应有短写法: {}",
        none.out
    );

    let help = run(t.path(), &["--help"]);
    assert_eq!(help.code, 0);
    assert!(help.out.starts_with("usage: snap "), "{}", help.out);

    let bogus = run(t.path(), &["bogus"]);
    assert_ne!(bogus.code, 0);
    assert!(bogus.all().contains("bogus"), "{}", bogus.all());

    let nover = run(t.path(), &["-c"]);
    assert_ne!(nover.code, 0, "缺少版本参数应报错");
}

// ---------------------------------------------------------------- 索引（跳过未变文件）

/// 索引存在时，同长度内容改动也必须被发现（靠大小+修改时间判断）
#[test]
fn index_detects_same_size_change() {
    let t = repo("idx-change");
    t.text("a.txt", "AAAA");
    t.text("b.txt", "BBBB");
    let v1 = save(t.path(), "v1");
    assert!(t.exists(".snap/index"), "保存后应写下索引");

    // 长度不变、只改内容
    t.text("a.txt", "ZZZZ");
    let o = run(t.path(), &["-t"]);
    assert!(o.out.contains("M a.txt"), "同长度改动必须被发现: {}", o.out);
    let v2 = save(t.path(), "v2");
    assert_eq!(t.tree(&v2), t.tree(&v1), "文件名集合不变");
    assert_ne!(
        t.version(&v2)["tree"]["a.txt"],
        t.version(&v1)["tree"]["a.txt"]
    );

    // 改回来也要能发现
    t.text("a.txt", "AAAA");
    let o = run(t.path(), &["-t"]);
    assert!(o.out.contains("M a.txt"), "改回来同样要发现: {}", o.out);
}

/// 索引损坏或缺失时，结果必须仍然正确（只是慢一点）
#[test]
fn index_corrupt_or_missing_is_safe() {
    let t = repo("idx-broken");
    t.text("a.txt", "A");
    let v1 = save(t.path(), "v1");

    // 塞入垃圾
    fs::write(t.path().join(".snap/index"), b"{ not json at all").unwrap();
    let o = run(t.path(), &["-t"]);
    assert!(
        o.out.contains("工作区干净"),
        "坏索引不能影响判断: {}",
        o.out
    );
    t.text("a.txt", "B");
    let o = run(t.path(), &["-t"]);
    assert!(
        o.out.contains("M a.txt"),
        "坏索引之后仍要能发现改动: {}",
        o.out
    );

    // 直接删掉
    fs::remove_file(t.path().join(".snap/index")).unwrap();
    let o = run(t.path(), &["-t"]);
    assert!(
        o.out.contains("M a.txt"),
        "没有索引也要能发现改动: {}",
        o.out
    );
    let v2 = save(t.path(), "v2");
    assert_ne!(v1, v2);
}

/// SNAP_NO_INDEX=1 时完全不用索引，结果一致
#[test]
fn index_can_be_disabled() {
    let t = repo("idx-off");
    t.text("a.txt", "A");
    save(t.path(), "v1");
    let before = fs::read_to_string(t.path().join(".snap/index")).unwrap_or_default();

    let out = Command::new(BIN)
        .args(["-t"])
        .current_dir(t.path())
        .env("SNAP_NO_INDEX", "1")
        .stdin(Stdio::null())
        .output()
        .expect("执行 snap");
    let o = convert(out);
    assert!(o.out.contains("工作区干净"), "{}", o.out);

    t.text("a.txt", "B");
    let out = Command::new(BIN)
        .args(["-t"])
        .current_dir(t.path())
        .env("SNAP_NO_INDEX", "1")
        .stdin(Stdio::null())
        .output()
        .expect("执行 snap");
    let o = convert(out);
    assert!(
        o.out.contains("M a.txt"),
        "禁用索引后仍要发现改动: {}",
        o.out
    );
    assert_eq!(
        fs::read_to_string(t.path().join(".snap/index")).unwrap_or_default(),
        before,
        "禁用索引时不应改写索引文件"
    );
}

/// 索引本身不能进版本库
#[test]
fn index_is_not_tracked() {
    let t = repo("idx-track");
    t.text("a.txt", "A");
    let v = save(t.path(), "v1");
    run(t.path(), &["-t"]); // 触发索引写入
    assert!(t.exists(".snap/index"), "索引应存在");
    let tree = t.tree(&v);
    assert!(
        !tree.iter().any(|k| k.starts_with(".snap")),
        "数据目录不得进版本: {tree:?}"
    );
    let o = run(t.path(), &["-t"]);
    assert!(
        o.out.contains("工作区干净"),
        "索引存在时工作区仍是干净的: {}",
        o.out
    );
}

/// checkout 之后索引要刷新：改回来立刻能发现，未改的保持干净
#[test]
fn checkout_refreshes_index() {
    let t = repo("idx-checkout");
    t.text("a.txt", "AAAAAAAA");
    let v1 = save(t.path(), "v1");
    t.text("a.txt", "BBBBBBBB"); // 同长度改动
    save(t.path(), "v2");

    let o = run_with_stdin(
        t.path(),
        &["-c", &v1[..6]],
        "y
",
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let clean = run(t.path(), &["-t"]);
    assert!(
        clean.out.contains("工作区干净"),
        "checkout 后必须干净: {}",
        clean.out
    );

    t.text("a.txt", "CCCCCCCC"); // 同长度改动
    let o = run(t.path(), &["-t"]);
    assert!(
        o.out.contains("M a.txt"),
        "checkout 刷新索引后仍要能发现改动: {}",
        o.out
    );
}

// ---------------------------------------------------------------- 分块并行压缩

/// 跨过分块阈值（8MB）的文件：保存 → 改坏 → 切回，必须逐字节一致
#[test]
fn large_file_chunked_roundtrip() {
    let t = repo("chunk-rt");
    // 12MB 随机数据（不可压缩，走"直接存储"分块）
    let mut bin = vec![0u8; 12 * 1024 * 1024];
    let mut seed: u64 = 0x1234_5678;
    for b in bin.iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *b = (seed >> 33) as u8;
    }
    t.file("big.bin", &bin);
    // 12MB 可压缩文本（走压缩分块）
    let text: String = "hello chunked world
"
    .repeat(12 * 1024 * 1024 / 20);
    t.file("big.txt", text.as_bytes());
    t.text("small.txt", "tiny");

    let v1 = save(t.path(), "v1");
    let tree = t.tree(&v1);
    assert!(tree.contains(&"big.bin".to_string()) && tree.contains(&"big.txt".to_string()));

    // 版本里记的哈希 = 文件真实 sha1（分块不影响对象命名）
    let want = {
        use sha1::{Digest, Sha1};
        let mut h = Sha1::new();
        h.update(&bin);
        format!("{:x}", h.finalize())
    };
    assert_eq!(t.version(&v1)["tree"]["big.bin"].as_str().unwrap(), want);

    // 改坏再切回
    t.file("big.bin", b"mutated");
    t.file("big.txt", b"mutated");
    let o = run_with_stdin(
        t.path(),
        &["-c", &v1[..6]],
        "y
",
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(t.read("big.bin"), bin, "分块存储的二进制必须逐字节还原");
    assert_eq!(
        t.read("big.txt"),
        text.as_bytes(),
        "分块压缩的文本必须逐字节还原"
    );
    assert_eq!(t.read("small.txt"), b"tiny");
}

/// 分块格式与旧的单流格式共存：小文件仍是单流，大文件是分块，互不影响
#[test]
fn mixed_object_formats_coexist() {
    let t = repo("chunk-mixed");
    t.text("small.txt", "single stream");
    let big = vec![7u8; 9 * 1024 * 1024];
    t.file("big.bin", &big);
    let v1 = save(t.path(), "v1");

    t.text("small.txt", "changed");
    t.file("big.bin", b"x");
    let o = run_with_stdin(
        t.path(),
        &["-c", &v1[..6]],
        "y
",
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(t.read("small.txt"), b"single stream");
    assert_eq!(t.read("big.bin"), big);
    let o = run(t.path(), &["-t"]);
    assert!(o.out.contains("工作区干净"), "{}", o.out);
}

/// SNAP_THREADS 可调，且不影响结果
#[test]
fn thread_count_is_configurable() {
    let t = repo("chunk-threads");
    let big = vec![3u8; 9 * 1024 * 1024];
    t.file("big.bin", &big);
    let out = Command::new(BIN)
        .args(["-s", "v1"])
        .current_dir(t.path())
        .env("SNAP_THREADS", "2")
        .stdin(Stdio::null())
        .output()
        .expect("执行 snap");
    let o = convert(out);
    assert_eq!(o.code, 0, "{}", o.all());
    let v = o.ids()[0].clone();

    t.file("big.bin", b"y");
    let o = run_with_env(
        t.path(),
        &["-c", &v[..6]],
        &[("SNAP_THREADS", "1")],
        "y
",
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(t.read("big.bin"), big, "线程数不影响结果");
}
