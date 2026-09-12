//! 仓库定位、对象库、版本读写、工作区扫描。
//!
//! 磁盘格式与 Python 版完全一致：
//!   .snap/HEAD                     当前版本ID（12位十六进制，或空）
//!   .snap/versions/<id>.json       版本记录 {parents, tree, message, time, seq}
//!   .snap/seq                      递增序号（同秒排序用）
//!   .snap/objects/<前2位>/<余38位>   zlib 压缩的文件内容，文件名为 sha1(原始内容)

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::SnapError;
use crate::ignore::{self, Pattern};

/// 压缩级别按文件大小分流（实测数据见 README 的性能一节）：
///   小文件多为源码/文本，级别 6 压得更小（100MB 文本: 33.2MB vs 级别 1 的 43.9MB）；
///   大文件多为镜像/媒体/压缩包这类已经压过的数据，级别 6 只是白烧 CPU
///   （4.5GB 镜像: 37.8 MB/s vs 级别 1 的 93.6 MB/s，体积只差 0.5%）。
pub const COMPRESSION_LEVEL_SMALL: u32 = 6;
pub const COMPRESSION_LEVEL_BIG: u32 = 1;
pub const BIG_FILE_BYTES: u64 = 4 << 20;

/// 多大以上的文件先探一段再决定压缩级别
pub const PROBE_MIN_BYTES: u64 = 4 << 20;
/// 大文件的压缩收益低于这个比例就直接存储（不值得为几个百分点烧 CPU）。
/// 0.10 = 省不到 10% 就只存储；调小更省空间，调大更快。
pub const SAVE_MIN_GAIN: f64 = 0.10;
/// 探测取样：段数 / 每段长度
pub const PROBE_SEGMENTS: usize = 16;
pub const PROBE_SEG_BYTES: usize = 64 << 10;

fn level_for(size: u64) -> u32 {
    if size >= BIG_FILE_BYTES {
        COMPRESSION_LEVEL_BIG
    } else {
        COMPRESSION_LEVEL_SMALL
    }
}

/// 判断一个文件压不压得动：在文件里均匀取若干小段分别试压，取收益的**中位数**。
///
/// 不能只看开头——ISO 这类文件头部元数据能压掉七八成，正文却是已压缩的包，
/// 只看开头会以为它很能压（实测：开头 1MB 可压 77%，整个文件只有 2.9%）。
/// 中位数能过滤掉这种局部很能压的干扰。压不动就直接存储（zlib 级别 0）。
fn pick_level_from(path: &Path, size: u64) -> u32 {
    if size < PROBE_MIN_BYTES {
        return level_for(size);
    }
    let mut gains: Vec<f64> = Vec::with_capacity(PROBE_SEGMENTS);
    if let Ok(mut f) = fs::File::open(path) {
        for i in 0..PROBE_SEGMENTS {
            let off = if size <= PROBE_SEG_BYTES as u64 {
                0
            } else {
                (size - PROBE_SEG_BYTES as u64) * i as u64 / (PROBE_SEGMENTS as u64 - 1)
            };
            if f.seek(SeekFrom::Start(off)).is_err() {
                break;
            }
            let mut buf = vec![0u8; PROBE_SEG_BYTES];
            let mut got = 0usize;
            while got < PROBE_SEG_BYTES {
                match f.read(&mut buf[got..]) {
                    Ok(0) => break,
                    Ok(n) => got += n,
                    Err(_) => break,
                }
            }
            if got < 1024 {
                break;
            }
            buf.truncate(got);
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(COMPRESSION_LEVEL_BIG));
            if enc.write_all(&buf).is_err() {
                break;
            }
            match enc.finish() {
                Ok(out) => gains.push(1.0 - out.len() as f64 / buf.len() as f64),
                Err(_) => break,
            }
        }
    }
    if gains.is_empty() {
        return level_for(size);
    }
    gains.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = gains[gains.len() / 2];
    if median < SAVE_MIN_GAIN {
        0
    } else {
        level_for(size)
    }
}

pub fn norm_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// 正在运行的这个程序自身（打包成 exe 后就是 exe 路径）。
pub fn self_path() -> Option<&'static PathBuf> {
    static SELF: OnceLock<Option<PathBuf>> = OnceLock::new();
    SELF.get_or_init(|| std::env::current_exe().ok()).as_ref()
}

/// rel 是否就是正在运行的这个程序文件（工具自己不进版本库）。
pub fn is_self_path(root: &Path, rel: &str) -> bool {
    let Some(sp) = self_path() else {
        return false;
    };
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let sp_name = sp
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name != sp_name {
        return false;
    }
    norm_key(&join_rel(root, rel)) == norm_key(sp)
}

/// 把正斜杠相对路径拼到根目录上。
pub fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

/// 相对路径 → 正斜杠形式（相对 base）。
pub fn rel_posix(base: &Path, p: &Path) -> Option<String> {
    let r = p.strip_prefix(base).ok()?;
    let mut out = String::new();
    for c in r.components() {
        let s = c.as_os_str().to_string_lossy();
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&s);
    }
    Some(out)
}

/// 校验版本里记录的相对路径，防止越界。
pub fn check_rel(rel: &str) -> Result<()> {
    if rel.is_empty() {
        return Err(SnapError::new(format!("版本里的路径非法: {rel:?}")));
    }
    let norm = rel.replace('\\', "/");
    if norm.starts_with('/') || Path::new(rel).is_absolute() || norm.split('/').any(|s| s == "..") {
        return Err(SnapError::new(format!("版本里的路径越界: {rel:?}")));
    }
    Ok(())
}

// ---------------------------------------------------------------- 仓库

pub const DATA_DIR: &str = ".snap";
pub const LEGACY_DIRS: [&str; 1] = [".vsnap"];
pub const IGNORE_FILES: [&str; 2] = [".snapignore", ".vsnapignore"];
pub const HELP_DOC_NAME: &str = "命令对照表.md";

pub type Result<T> = std::result::Result<T, SnapError>;

/// 目录 d 自己是不是仓库；是则返回数据目录名（兼容旧名）。
pub fn data_dir_here(d: &Path) -> Option<String> {
    for name in [DATA_DIR].iter().chain(LEGACY_DIRS.iter()) {
        if d.join(name).is_dir() {
            return Some((*name).to_string());
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct Repo {
    pub root: PathBuf,
    pub dir: String, // 数据目录名（.snap 或旧名 .vsnap）
}

impl Repo {
    /// 从 start（默认当前目录）向上找仓库。
    pub fn discover(start: &Path) -> Option<Repo> {
        let mut d = Some(start.to_path_buf());
        while let Some(cur) = d {
            if let Some(dir) = data_dir_here(&cur) {
                return Some(Repo { root: cur, dir });
            }
            d = cur.parent().map(|p| p.to_path_buf());
        }
        None
    }

    /// 当前目录所属的仓库；不在仓库里时退化成 cwd + 默认目录名。
    pub fn current() -> Repo {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Repo::discover(&cwd).unwrap_or(Repo {
            root: cwd,
            dir: DATA_DIR.to_string(),
        })
    }

    pub fn is_initialized(&self) -> bool {
        self.data_path(&[]).is_dir()
    }

    pub fn data_path(&self, parts: &[&str]) -> PathBuf {
        let mut p = self.root.join(&self.dir);
        for s in parts {
            p.push(s);
        }
        p
    }

    /// 工作区里的绝对路径。
    pub fn work_path(&self, rel: &str) -> PathBuf {
        join_rel(&self.root, rel)
    }

    /// 把旧名字的数据目录改成新名字（只在目标不存在时动手）。
    pub fn migrate_legacy_dir(&self) -> String {
        if self.dir == DATA_DIR {
            return self.dir.clone();
        }
        let new = self.root.join(DATA_DIR);
        if new.exists() {
            return self.dir.clone();
        }
        match fs::rename(self.root.join(&self.dir), &new) {
            Ok(()) => {
                println!("已把数据目录 {} 改名为 {}", self.dir, DATA_DIR);
                DATA_DIR.to_string()
            }
            Err(e) => {
                println!(
                    "提示: 无法把 {} 改名为 {}（{}），继续使用 {}",
                    self.dir, DATA_DIR, e, self.dir
                );
                self.dir.clone()
            }
        }
    }

    // ------------------------------------------------------------ 对象库

    pub fn object_path(&self, h: &str) -> PathBuf {
        self.data_path(&["objects", &h[..2], &h[2..]])
    }

    /// 内容寻址写入。相同内容永远返回同一个哈希，只存一份。
    ///
    /// 边读边压缩，内存占用与文件大小无关（大文件比如 ISO 也不会撑爆内存）。
    /// 先写临时文件，读完才知道哈希，再改名到最终位置。
    pub fn store_object_from_file(&self, src: &Path) -> Result<String> {
        let obj_root = self.data_path(&["objects"]);
        fs::create_dir_all(&obj_root)
            .map_err(|e| SnapError::new(format!("无法创建对象目录: {e}")))?;
        let tmp = obj_root.join(format!(".tmp-{}-{}", std::process::id(), tmp_counter()));

        let size = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
        let level = pick_level_from(src, size);

        // 大文件：分块并行压缩，单线程 zlib 的瓶颈交给多核
        if size >= crate::chunk::CHUNK_MIN_BYTES {
            let threads = crate::chunk::thread_count();
            let hash = crate::chunk::write_chunked(src, &tmp, level, threads)?;
            let dest = self.object_path(&hash);
            if dest.exists() {
                quiet_remove(&tmp);
            } else {
                if let Some(d) = dest.parent() {
                    fs::create_dir_all(d)
                        .map_err(|e| SnapError::new(format!("无法创建对象目录: {e}")))?;
                }
                if let Err(e) = fs::rename(&tmp, &dest) {
                    quiet_remove(&tmp);
                    return Err(SnapError::new(format!("无法写入对象 {}…: {e}", &hash[..8])));
                }
            }
            return Ok(hash);
        }

        let mut h = Sha1::new();
        {
            let fin = fs::File::open(src)
                .map_err(|e| SnapError::new(format!("无法读取 {}: {e}", src.display())))?;
            let mut fin = std::io::BufReader::new(fin);
            let fout = fs::File::create(&tmp)
                .map_err(|e| SnapError::new(format!("无法创建临时对象: {e}")))?;
            let mut enc = ZlibEncoder::new(std::io::BufWriter::new(fout), Compression::new(level));
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = fin
                    .read(&mut buf)
                    .map_err(|e| SnapError::new(format!("无法读取 {}: {e}", src.display())))?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
                enc.write_all(&buf[..n])
                    .map_err(|e| SnapError::new(format!("压缩失败: {e}")))?;
            }
            let mut bw = enc
                .finish()
                .map_err(|e| SnapError::new(format!("压缩失败: {e}")))?;
            bw.flush()
                .map_err(|e| SnapError::new(format!("写入对象失败: {e}")))?;
        }

        let hash = hex(&h.finalize());
        let dest = self.object_path(&hash);
        if dest.exists() {
            quiet_remove(&tmp); // 相同内容已经在库里了
        } else {
            if let Some(d) = dest.parent() {
                fs::create_dir_all(d)
                    .map_err(|e| SnapError::new(format!("无法创建对象目录: {e}")))?;
            }
            if let Err(e) = fs::rename(&tmp, &dest) {
                quiet_remove(&tmp);
                return Err(SnapError::new(format!("无法写入对象 {}…: {e}", &hash[..8])));
            }
        }
        Ok(hash)
    }

    /// 把对象解压后直接写进目标文件（流式，内存占用恒定）。
    pub fn checkout_object_to(&self, h: &str, dest: &Path) -> Result<()> {
        let src = self.object_path(h);
        let fin = fs::File::open(&src).map_err(|_| {
            SnapError::new(format!(
                "对象库缺少对象 {}…，该版本无法完整还原（可能被误删或磁盘损坏，可检查 {}）",
                &h[..8],
                self.data_path(&["objects"]).display()
            ))
        })?;
        if crate::chunk::is_chunked(&src) {
            // 分块容器：走另一条读路径（旧仓库里的单条 zlib 流仍然照常处理）
            drop(fin);
            return crate::chunk::read_chunked(&src, dest);
        }
        let mut dec = ZlibDecoder::new(std::io::BufReader::new(fin));
        let out = fs::File::create(dest)
            .map_err(|e| SnapError::new(format!("无法写入 {}: {e}", dest.display())))?;
        let mut out = std::io::BufWriter::new(out);
        std::io::copy(&mut dec, &mut out)
            .map_err(|e| SnapError::new(format!("对象 {}… 解压失败: {e}", &h[..8])))?;
        out.flush()
            .map_err(|e| SnapError::new(format!("无法写入 {}: {e}", dest.display())))?;
        Ok(())
    }

    pub fn object_exists(&self, h: &str) -> bool {
        self.object_path(h).is_file()
    }

    // ------------------------------------------------------------ HEAD

    pub fn head_path(&self) -> PathBuf {
        self.data_path(&["HEAD"])
    }

    pub fn read_head(&self) -> Result<Option<String>> {
        let p = self.head_path();
        if !p.exists() {
            return Ok(None);
        }
        let s =
            fs::read_to_string(&p).map_err(|e| SnapError::new(format!("无法读取 HEAD: {e}")))?;
        let s = s.trim().to_string();
        Ok(if s.is_empty() { None } else { Some(s) })
    }

    pub fn write_head(&self, id: Option<&str>) -> Result<()> {
        fs::write(self.head_path(), id.unwrap_or(""))
            .map_err(|e| SnapError::new(format!("无法写入 HEAD: {e}")))
    }

    pub fn next_seq(&self) -> i64 {
        let p = self.data_path(&["seq"]);
        let cur: i64 = fs::read_to_string(&p)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let next = cur + 1;
        let _ = fs::write(&p, next.to_string());
        next
    }

    // ------------------------------------------------------------ 版本

    pub fn version_path(&self, id: &str) -> PathBuf {
        self.data_path(&["versions", &format!("{id}.json")])
    }

    pub fn load_version(&self, id: &str) -> Result<Version> {
        let p = self.version_path(id);
        if !p.is_file() {
            return Err(SnapError::new(format!("版本不存在: {id}")));
        }
        let text = fs::read_to_string(&p)
            .map_err(|e| SnapError::new(format!("版本文件损坏: {id} ({e})")))?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| SnapError::new(format!("版本文件损坏: {id} ({e})")))?;
        let tree_val = v
            .get("tree")
            .and_then(|t| t.as_object())
            .ok_or_else(|| SnapError::new(format!("版本文件损坏: {id} (缺少 tree 字段)")))?;

        let mut tree = BTreeMap::new();
        for (k, hv) in tree_val {
            let Some(h) = hv.as_str() else { continue };
            // 旧版本可能把平台分隔符写进了路径；工具自己不进版本
            let key = k.replace('\\', "/");
            if is_self_path(&self.root, &key) {
                continue;
            }
            tree.insert(key, h.to_string());
        }

        Ok(Version {
            id: id.to_string(),
            parents: v
                .get("parents")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            tree,
            message: v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string(),
            time: v.get("time").and_then(|t| t.as_i64()).unwrap_or(0),
            seq: v.get("seq").and_then(|s| s.as_i64()).unwrap_or(0),
        })
    }

    /// 所有版本，按 (seq, time, id) 升序；读不出来的文件跳过。
    pub fn all_versions(&self) -> Vec<Version> {
        let dir = self.data_path(&["versions"]);
        let Ok(rd) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            if let Ok(v) = self.load_version(id) {
                out.push(v);
            }
        }
        out.sort_by(|a, b| (a.seq, a.time, &a.id).cmp(&(b.seq, b.time, &b.id)));
        out
    }

    /// 版本ID前缀 → 完整ID，要求唯一。
    pub fn resolve(&self, prefix: &str) -> Result<String> {
        let prefix = prefix.trim();
        if prefix.is_empty() {
            return Err(SnapError::new("请给出要操作的版本ID（可以只写前几位前缀）"));
        }
        let hits: Vec<String> = self
            .all_versions()
            .into_iter()
            .map(|v| v.id)
            .filter(|id| id.starts_with(prefix))
            .collect();
        match hits.len() {
            0 => Err(SnapError::new(format!("找不到版本: {prefix}"))),
            1 => Ok(hits.into_iter().next().unwrap()),
            _ => Err(SnapError::new(format!(
                "版本ID不唯一: {prefix} -> {}",
                hits.join(", ")
            ))),
        }
    }

    // ------------------------------------------------------------ 扫描

    /// 工作区里未被忽略的文件（正斜杠相对路径，已排序）。
    pub fn iter_files(&self, pats: &[Pattern]) -> Result<Vec<String>> {
        let mut out = Vec::new();
        self.walk(&self.root.clone(), "", pats, &mut out)?;
        out.sort();
        Ok(out)
    }

    fn walk(
        &self,
        dir: &Path,
        rel_dir: &str,
        pats: &[Pattern],
        out: &mut Vec<String>,
    ) -> Result<()> {
        let rd = fs::read_dir(dir)
            .map_err(|e| SnapError::new(format!("无法读取目录 {}: {e}", dir.display())))?;
        let mut entries: Vec<(String, PathBuf)> = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.file_name().to_str().is_none() {
                eprintln!("警告: 跳过文件名不是有效 UTF-8 的条目");
                continue;
            }
            entries.push((name, e.path()));
        }
        entries.sort();

        for (name, path) in &entries {
            let rel = if rel_dir.is_empty() {
                name.clone()
            } else {
                format!("{rel_dir}/{name}")
            };
            let md = fs::symlink_metadata(path)
                .map_err(|e| SnapError::new(format!("无法读取 {rel}: {e}")))?;
            let is_link = md.file_type().is_symlink();
            let is_dir = if is_link { path.is_dir() } else { md.is_dir() };
            if is_link && is_dir {
                continue; // 与 Python 的 os.walk(followlinks=False) 一致：不跟进目录软链
            }
            if is_dir {
                if ignore::is_ignored(&rel, pats, true) {
                    continue;
                }
                self.walk(path, &rel, pats, out)?;
            } else {
                if ignore::is_ignored(&rel, pats, false) || is_self_path(&self.root, &rel) {
                    continue;
                }
                out.push(rel);
            }
        }
        Ok(())
    }

    /// 工作区 → {相对路径: sha1(内容)}。没变过的文件走索引，不重新读盘。
    pub fn workspace_hashes(&self) -> Result<BTreeMap<String, String>> {
        let pats = self.load_ignore()?;
        let files = self.iter_files(&pats)?;
        let mut index = Index::load(self);
        let mut out = BTreeMap::new();
        let total = files.len();
        let mut last = std::time::Instant::now();
        for (i, rel) in files.iter().enumerate() {
            out.insert(rel.clone(), index.hash_of(self, rel)?);
            if last.elapsed().as_secs() >= 1 {
                crate::winout::progress(&format!("扫描中 {}/{} 个文件", i + 1, total));
                last = std::time::Instant::now();
            }
        }
        crate::winout::progress_done();
        let _ = index.save(self); // 索引只是加速用的，写不了也不影响结果
        Ok(out)
    }

    /// 工作区 → {相对路径: blob哈希}，并把内容写入对象库。
    ///
    /// 大仓库会边扫边在 stderr 上报进度（只有真的终端才显示，管道/重定向时不打扰）。
    pub fn scan_tree(&self) -> Result<BTreeMap<String, String>> {
        let pats = self.load_ignore()?;
        let files = self.iter_files(&pats)?;
        let total = files.len();
        let mut index = Index::load(self);
        let mut tree = BTreeMap::new();
        let mut bytes = 0u64;
        let mut reused = 0usize;
        let mut last = std::time::Instant::now();

        for (i, rel) in files.iter().enumerate() {
            check_rel(rel)?;
            let path = self.work_path(rel);
            let (size, mtime, ctime) = file_stamp(&path)?;
            // 没变过、对象也还在，就直接沿用：连读盘和压缩都省了
            let hash = match index.cached(rel, size, mtime, ctime) {
                Some(h) if self.object_exists(&h) => {
                    reused += 1;
                    h
                }
                _ => self.store_object_from_file(&path)?,
            };
            bytes += size;
            tree.insert(rel.clone(), hash);
            if last.elapsed().as_secs() >= 1 {
                let extra = if reused > 0 {
                    format!(", 跳过 {reused} 个未变文件")
                } else {
                    String::new()
                };
                crate::winout::progress(&format!(
                    "扫描中 {}/{} 个文件, {:.2} GB{extra}",
                    i + 1,
                    total,
                    bytes as f64 / 1e9
                ));
                last = std::time::Instant::now();
            }
        }
        crate::winout::progress_done();
        index.reset_to(self, &tree);
        let _ = index.save(self);
        Ok(tree)
    }

    /// 规则 = 默认忽略 + .snapignore（找不到则用旧名 .vsnapignore）。
    pub fn load_ignore(&self) -> Result<Vec<Pattern>> {
        let mut lines: Vec<String> = ignore::DEFAULT_IGNORE
            .iter()
            .map(|s| s.to_string())
            .collect();
        for name in IGNORE_FILES {
            let p = self.root.join(name);
            if !p.is_file() {
                continue;
            }
            let text = fs::read_to_string(&p)
                .map_err(|e| SnapError::new(format!("无法读取 {name}: {e}")))?;
            lines.extend(text.lines().map(|s| s.to_string()));
            break;
        }
        Ok(ignore::compile(lines.iter().map(|s| s.as_str())))
    }
}

/// 读文件并算 sha1（分块，避免大文件全读进内存）。
pub fn file_sha1(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)
        .map_err(|e| SnapError::new(format!("无法读取 {}: {e}", path.display())))?;
    let mut h = Sha1::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| SnapError::new(format!("无法读取 {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub fn sha1_hex(data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(data);
    hex(&h.finalize())
}

/// 临时对象文件的序号，避免并发/连写时撞名。
fn tmp_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn quiet_remove(p: &Path) {
    let _ = fs::remove_file(p);
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------- 索引

/// `.snap/index`：记录每个文件的 (大小, 修改时间, 内容哈希)。
///
/// 判断文件有没有变过的依据是「大小 + 修改时间」，思路和 git 的索引一样，
/// 目的是让大目录的 status / save / checkout 不用每次重新哈希全部内容。
///
/// 局限：如果某个工具在改内容的同时把修改时间也保留下来（`touch -r`、
/// `cp -p` 之类），这一次改动会被漏掉。设 `SNAP_NO_INDEX=1` 可强制每次都重算。
pub struct Index {
    /// rel -> (大小, 修改时间, 变更时间, 内容哈希)
    entries: HashMap<String, (u64, i64, i64, String)>,
    /// 索引写入时刻：修改时间不早于它的条目不可信（git 的 racy 规则）
    written: i64,
    dirty: bool,
}

#[cfg(windows)]
mod win_ctime {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            hfile: *mut c_void,
            fileinformationclass: i32,
            lpfileinformation: *mut c_void,
            dwbuffersize: u32,
        ) -> i32;
    }

    #[repr(C)]
    struct FileBasicInfo {
        creation_time: i64,
        last_access_time: i64,
        last_write_time: i64,
        change_time: i64,
        file_attributes: u32,
    }

    /// 文件的"变更时间"（100ns 刻度，自 1601 起）。取不到返回 None。
    pub fn change_ticks(path: &std::path::Path) -> Option<i64> {
        let f = std::fs::File::open(path).ok()?;
        let mut info = FileBasicInfo {
            creation_time: 0,
            last_access_time: 0,
            last_write_time: 0,
            change_time: 0,
            file_attributes: 0,
        };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                f.as_raw_handle(),
                0, // FileBasicInfo
                &mut info as *mut FileBasicInfo as *mut c_void,
                std::mem::size_of::<FileBasicInfo>() as u32,
            )
        };
        if ok == 0 {
            None
        } else {
            Some(info.change_time)
        }
    }
}

/// 变更时间：Unix 上从 stat 免费拿到；Windows 上要开一次句柄。
#[cfg(unix)]
fn change_time(_path: &Path, md: &fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    md.ctime()
        .saturating_mul(1_000_000_000)
        .saturating_add(md.ctime_nsec())
}

#[cfg(windows)]
fn change_time(path: &Path, _md: &fs::Metadata) -> i64 {
    win_ctime::change_ticks(path).unwrap_or(0) // 0 = 拿不到，此时不信任缓存
}

#[cfg(not(any(unix, windows)))]
fn change_time(_path: &Path, _md: &fs::Metadata) -> i64 {
    0
}

fn mtime_ns(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// 文件戳：(大小, 修改时间, 变更时间)。
fn file_stamp(path: &Path) -> Result<(u64, i64, i64)> {
    let md = fs::metadata(path)
        .map_err(|e| SnapError::new(format!("无法读取 {}: {e}", path.display())))?;
    Ok((md.len(), mtime_ns(&md), change_time(path, &md)))
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

impl Index {
    /// 设了 SNAP_NO_INDEX 就完全不使用索引。
    pub fn disabled() -> bool {
        std::env::var_os("SNAP_NO_INDEX").is_some()
    }

    pub fn load(repo: &Repo) -> Index {
        let mut idx = Index {
            entries: HashMap::new(),
            written: 0,
            dirty: false,
        };
        if Index::disabled() {
            return idx;
        }
        // 没有索引、或者索引坏了，都当成空的：只是慢一点，结果一样
        let Ok(text) = fs::read_to_string(repo.data_path(&["index"])) else {
            return idx;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            return idx;
        };
        idx.written = v.get("written").and_then(|w| w.as_i64()).unwrap_or(0);
        if let Some(map) = v.get("entries").and_then(|e| e.as_object()) {
            for (k, val) in map {
                let Some(a) = val.as_array() else { continue };
                let size = a.first().and_then(|x| x.as_u64());
                let mtime = a.get(1).and_then(|x| x.as_i64());
                let ctime = a.get(2).and_then(|x| x.as_i64());
                let hash = a.get(3).and_then(|x| x.as_str());
                // 旧版索引没有 ctime，直接忽略这条，让它重算一次即可
                if let (Some(size), Some(mtime), Some(ctime), Some(hash)) =
                    (size, mtime, ctime, hash)
                    && ctime > 0
                {
                    idx.entries
                        .insert(k.clone(), (size, mtime, ctime, hash.to_string()));
                }
            }
        }
        idx
    }

    /// 内容没变的话直接给出哈希，不用读文件。
    ///
    /// 三个条件都要满足：大小、修改时间、变更时间都与记录一致；
    /// 而且修改时间必须**早于**索引写入时刻（git 的 racy 规则）——否则说明它可能是
    /// 在"算哈希"与"写索引"之间被改过，不能信。
    pub fn cached(&self, rel: &str, size: u64, mtime: i64, ctime: i64) -> Option<String> {
        if ctime <= 0 || mtime >= self.written {
            return None;
        }
        self.entries
            .get(rel)
            .filter(|(s, m, c, _)| *s == size && *m == mtime && *c == ctime)
            .map(|(_, _, _, h)| h.clone())
    }

    pub fn put(&mut self, rel: &str, size: u64, mtime: i64, ctime: i64, hash: &str) {
        self.entries
            .insert(rel.to_string(), (size, mtime, ctime, hash.to_string()));
        self.dirty = true;
    }

    /// 算一个文件的内容哈希，能用索引就用。
    pub fn hash_of(&mut self, repo: &Repo, rel: &str) -> Result<String> {
        let path = repo.work_path(rel);
        let (size, mtime, ctime) = file_stamp(&path)?;
        if let Some(h) = self.cached(rel, size, mtime, ctime) {
            return Ok(h);
        }
        let h = file_sha1(&path)?;
        self.put(rel, size, mtime, ctime, &h);
        Ok(h)
    }

    /// 用已知的树（rel → 内容哈希）重建索引：save / checkout 之后工作区就是这棵树，
    /// 不必再重新哈希一遍。
    pub fn reset_to(&mut self, repo: &Repo, tree: &BTreeMap<String, String>) {
        self.entries.clear();
        for (rel, hash) in tree {
            if let Ok((size, mtime, ctime)) = file_stamp(&repo.work_path(rel)) {
                self.entries
                    .insert(rel.clone(), (size, mtime, ctime, hash.clone()));
            }
        }
        self.dirty = true;
    }

    pub fn save(&self, repo: &Repo) -> Result<()> {
        if !self.dirty || Index::disabled() {
            return Ok(());
        }
        let mut entries = serde_json::Map::new();
        for (k, (size, mtime, ctime, hash)) in &self.entries {
            entries.insert(k.clone(), serde_json::json!([size, mtime, ctime, hash]));
        }
        let mut root = serde_json::Map::new();
        root.insert("version".into(), serde_json::json!(2));
        root.insert("written".into(), serde_json::json!(now_ns()));
        root.insert("entries".into(), Value::Object(entries));
        let text = serde_json::to_string(&Value::Object(root))
            .map_err(|e| SnapError::new(format!("无法生成索引: {e}")))?;
        let path = repo.data_path(&["index"]);
        let tmp = repo.data_path(&["index.tmp"]);
        fs::write(&tmp, text).map_err(|e| SnapError::new(format!("无法写入索引: {e}")))?;
        fs::rename(&tmp, &path).map_err(|e| SnapError::new(format!("无法写入索引: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------- 版本数据

#[derive(Clone, Debug)]
pub struct Version {
    pub id: String,
    pub parents: Vec<String>,
    pub tree: BTreeMap<String, String>,
    pub message: String,
    pub time: i64,
    pub seq: i64,
}

/// 版本文件内容（键排序、2 空格缩进，与 Python 的 json.dump 一致）。
pub fn version_json(v: &Version) -> String {
    let mut s = String::from("{\n");
    s.push_str(&format!("  \"message\": {},\n", json_string(&v.message)));
    if v.parents.is_empty() {
        s.push_str("  \"parents\": [],\n");
    } else {
        s.push_str("  \"parents\": [\n");
        for (i, p) in v.parents.iter().enumerate() {
            let comma = if i + 1 < v.parents.len() { "," } else { "" };
            s.push_str(&format!("    {}{}\n", json_string(p), comma));
        }
        s.push_str("  ],\n");
    }
    s.push_str(&format!("  \"seq\": {},\n", v.seq));
    s.push_str(&format!("  \"time\": {},\n", v.time));
    if v.tree.is_empty() {
        s.push_str("  \"tree\": {}\n");
    } else {
        s.push_str("  \"tree\": {\n");
        let n = v.tree.len();
        for (i, (k, h)) in v.tree.iter().enumerate() {
            let comma = if i + 1 < n { "," } else { "" };
            s.push_str(&format!(
                "    {}: {}{}\n",
                json_string(k),
                json_string(h),
                comma
            ));
        }
        s.push_str("  }\n");
    }
    s.push('}');
    s
}

/// 版本ID的取值依据：Python 的
/// json.dumps({"parents":..,"tree":..,"message":..}, sort_keys=True, ensure_ascii=False)
/// 这里逐字节复刻它的输出（键排序、", " 与 ": " 分隔、非 ASCII 不转义）。
pub fn id_payload(parents: &[String], tree: &BTreeMap<String, String>, message: &str) -> String {
    let mut s = String::from("{\"message\": ");
    s.push_str(&json_string(message));
    s.push_str(", \"parents\": [");
    for (i, p) in parents.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&json_string(p));
    }
    s.push_str("], \"tree\": {");
    for (i, (k, h)) in tree.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("{}: {}", json_string(k), json_string(h)));
    }
    s.push_str("}}");
    s
}

/// 版本ID = sha1(载荷) 的前 12 位。
pub fn version_id(parents: &[String], tree: &BTreeMap<String, String>, message: &str) -> String {
    let h = sha1_hex(id_payload(parents, tree, message).as_bytes());
    h[..12].to_string()
}

/// JSON 字符串字面量，转义规则与 Python 的 json.dumps(ensure_ascii=False) 一致。
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn id_payload_format() {
        // 载荷格式固定：键排序、", " 与 ": " 分隔、非 ASCII 不转义。
        // 这样任何按同样规则实现的工具都能算出同一个版本ID。
        let p = id_payload(&[], &tree(&[("a.txt", "h")]), "v1");
        assert_eq!(
            p,
            r#"{"message": "v1", "parents": [], "tree": {"a.txt": "h"}}"#
        );
    }

    #[test]
    fn payload_with_parents_and_unicode() {
        let p = id_payload(&["abc".to_string()], &tree(&[("中文.txt", "h")]), "说明");
        assert_eq!(
            p,
            r#"{"message": "说明", "parents": ["abc"], "tree": {"中文.txt": "h"}}"#
        );
    }

    #[test]
    fn json_escaping() {
        // 转义规则：引号与反斜杠加转义、控制字符用 \n \t 等或 \uXXXX，其余保持原样
        assert_eq!(json_string("a\"b\\c\nd"), r#""a\"b\\c\nd""#);
        assert_eq!(json_string("\u{1}"), r#""\u0001""#);
    }

    #[test]
    fn version_json_layout() {
        let v = Version {
            id: "x".into(),
            parents: vec![],
            tree: tree(&[("a.txt", "h")]),
            message: "v1".into(),
            time: 100,
            seq: 1,
        };
        let s = version_json(&v);
        assert!(s.contains("\"seq\": 1,\n"), "{s}");
        assert!(
            s.contains("  \"tree\": {\n    \"a.txt\": \"h\"\n  }\n"),
            "{s}"
        );
    }
}
