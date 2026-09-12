//! 分块并行压缩：大文件按 4MB 分块、多线程压缩后装进一个容器。
//!
//! 为什么需要：zlib 是单线程的，压缩是保存时的瓶颈。把文件切成块之后可以多个核
//! 一起压，吞吐基本随核数线性上涨。解压仍然很快，所以读路径保持流式顺序处理。
//!
//! 容器格式（小端）：
//! ```text
//!   0   : magic "SNPCHUNK" (8 字节)
//!   8   : version     u32 = 1
//!   12  : chunk_size  u32      每块原始字节数
//!   16  : count       u64      块数
//!   24  : table[count] × 16    每块: 数据偏移 u64, 压缩长度 u32, 原始长度 u32
//!   之后: 各块压缩数据（按完成顺序写入，偏移由表中记录）
//! ```
//!
//! 对象名仍然是 sha1(原始内容)，所以版本里记录的哈希、去重逻辑都不受影响；
//! 旧仓库里的"单条 zlib 流"对象也照常能读（读的时候先嗅探 magic）。

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc::sync_channel;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use sha1::{Digest, Sha1};

use crate::SnapError;
use crate::store::Result;

pub const MAGIC: &[u8; 8] = b"SNPCHUNK";
pub const VERSION: u32 = 1;
/// 每块原始字节数
pub const CHUNK_BYTES: usize = 4 << 20;
/// 大于等于这个大小才走分块并行（小文件并行没意义，反而多一层开销）
pub const CHUNK_MIN_BYTES: u64 = 8 << 20;
/// 表项大小：偏移 8 + 压缩长度 4 + 原始长度 4
const ENTRY: usize = 16;
const PREAMBLE: usize = 8 + 4 + 4 + 8;

/// 默认线程数：取 CPU 并行度，最多 8（再往上收益递减，反而更吃内存）；
/// 可用 SNAP_THREADS 覆盖。
pub fn thread_count() -> usize {
    if let Ok(v) = std::env::var("SNAP_THREADS")
        && let Ok(n) = v.trim().parse::<usize>()
    {
        return n.clamp(1, 64);
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8)
}

/// 这个对象文件是分块容器吗？
pub fn is_chunked(path: &Path) -> bool {
    let mut buf = [0u8; 8];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
        && &buf == MAGIC
}

struct Shared {
    file: File,
    /// (数据偏移, 压缩长度, 原始长度)
    table: Vec<(u64, u32, u32)>,
    data_off: u64,
}

/// 把 src 分块并行压缩写入 dst；返回原始内容的 sha1。
pub fn write_chunked(src: &Path, dst: &Path, level: u32, threads: usize) -> Result<String> {
    let size = fs::metadata(src)
        .map_err(|e| SnapError::new(format!("无法读取 {}: {e}", src.display())))?
        .len();
    let count = size.div_ceil(CHUNK_BYTES as u64) as usize;
    let header_len = PREAMBLE + count * ENTRY;
    let threads = threads.max(1);

    let file = File::create(dst).map_err(|e| SnapError::new(format!("无法创建临时对象: {e}")))?;
    file.set_len(header_len as u64 + size) // 先按最坏情况预留，最后按实际长度截断
        .map_err(|e| SnapError::new(format!("无法创建临时对象: {e}")))?;

    let shared = Mutex::new(Shared {
        file,
        table: vec![(0, 0, 0); count],
        data_off: header_len as u64,
    });
    let (tx, rx) = sync_channel::<(usize, Vec<u8>)>(threads * 2);
    let rx = Mutex::new(rx);
    let failure: Mutex<Option<String>> = Mutex::new(None);

    let hash = std::thread::scope(|scope| -> Result<String> {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let job = {
                        let guard = rx.lock().unwrap_or_else(|e| e.into_inner());
                        guard.recv()
                    };
                    let Ok((idx, raw)) = job else { break };
                    let mut enc = ZlibEncoder::new(
                        Vec::with_capacity(raw.len() / 2 + 64),
                        Compression::new(level),
                    );
                    let compressed = match enc.write_all(&raw).and_then(|_| enc.finish()) {
                        Ok(c) => c,
                        Err(e) => {
                            let mut f = failure.lock().unwrap_or_else(|e| e.into_inner());
                            *f = Some(format!("压缩失败: {e}"));
                            continue; // 继续收下后续的块，别让生产者卡住
                        }
                    };
                    let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                    let off = st.data_off;
                    let ok = st
                        .file
                        .seek(SeekFrom::Start(off))
                        .and_then(|_| st.file.write_all(&compressed))
                        .is_ok();
                    if !ok {
                        let mut f = failure.lock().unwrap_or_else(|e| e.into_inner());
                        *f = Some("写入临时对象失败".to_string());
                        continue;
                    }
                    st.table[idx] = (off, compressed.len() as u32, raw.len() as u32);
                    st.data_off = off + compressed.len() as u64;
                }
            });
        }

        // 主线程顺序读 + 算哈希，把块交给上面这些工作线程
        let reader = File::open(src)
            .map_err(|e| SnapError::new(format!("无法读取 {}: {e}", src.display())))?;
        let mut reader = BufReader::new(reader);
        let mut hasher = Sha1::new();
        let mut buf = vec![0u8; CHUNK_BYTES];
        let mut idx = 0usize;
        loop {
            let mut got = 0usize;
            while got < CHUNK_BYTES {
                match reader.read(&mut buf[got..]) {
                    Ok(0) => break,
                    Ok(n) => got += n,
                    Err(e) => {
                        return Err(SnapError::new(format!("无法读取 {}: {e}", src.display())));
                    }
                }
            }
            if got == 0 {
                break;
            }
            hasher.update(&buf[..got]);
            if tx.send((idx, buf[..got].to_vec())).is_err() {
                break; // 工作线程提前退出，后面会报错
            }
            idx += 1;
        }
        // 关键：必须在 scope 结束前关掉发送端，否则工作线程会一直阻塞在 recv()
        // 上，而 thread::scope 又要等它们结束 —— 互相等就死锁了。
        drop(tx);
        Ok(sha1_hex_from(hasher))
    })?;

    let mut st = shared.into_inner().unwrap_or_else(|e| e.into_inner());
    if let Some(msg) = failure.into_inner().unwrap_or_else(|e| e.into_inner()) {
        return Err(SnapError::new(msg));
    }
    // 所有块都要写完整，否则宁可失败
    if let Some(i) = st.table.iter().position(|&(_, _, rl)| rl == 0) {
        return Err(SnapError::new(format!("有分块没有写完（第 {} 块）", i + 1)));
    }

    // 回填头部并截断到真实长度
    let mut head = Vec::with_capacity(header_len);
    head.extend_from_slice(MAGIC);
    head.extend_from_slice(&VERSION.to_le_bytes());
    head.extend_from_slice(&(CHUNK_BYTES as u32).to_le_bytes());
    head.extend_from_slice(&(count as u64).to_le_bytes());
    for &(off, cl, rl) in &st.table {
        head.extend_from_slice(&off.to_le_bytes());
        head.extend_from_slice(&cl.to_le_bytes());
        head.extend_from_slice(&rl.to_le_bytes());
    }
    st.file
        .seek(SeekFrom::Start(0))
        .and_then(|_| st.file.write_all(&head))
        .and_then(|_| st.file.set_len(st.data_off))
        .map_err(|e| SnapError::new(format!("无法写入对象头部: {e}")))?;
    drop(st);

    Ok(hash)
}

fn sha1_hex_from(hasher: Sha1) -> String {
    let d = hasher.finalize();
    let mut s = String::with_capacity(40);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 把分块容器解压写出到 dst（一次只驻留一个块，内存与文件大小无关）。
pub fn read_chunked(src: &Path, dst: &Path) -> Result<()> {
    let mut f = File::open(src)
        .map_err(|e| SnapError::new(format!("无法读取对象 {}: {e}", src.display())))?;
    let mut head = [0u8; PREAMBLE];
    f.read_exact(&mut head)
        .map_err(|e| SnapError::new(format!("对象头部损坏: {e}")))?;
    if &head[..8] != MAGIC {
        return Err(SnapError::new("不是分块对象"));
    }
    let count = u64::from_le_bytes(head[16..24].try_into().unwrap()) as usize;

    let mut table = Vec::with_capacity(count);
    let mut ent = [0u8; ENTRY];
    for _ in 0..count {
        f.read_exact(&mut ent)
            .map_err(|e| SnapError::new(format!("对象表损坏: {e}")))?;
        let off = u64::from_le_bytes(ent[0..8].try_into().unwrap());
        let cl = u32::from_le_bytes(ent[8..12].try_into().unwrap()) as usize;
        let rl = u32::from_le_bytes(ent[12..16].try_into().unwrap()) as usize;
        table.push((off, cl, rl));
    }

    let out = File::create(dst)
        .map_err(|e| SnapError::new(format!("无法写入 {}: {e}", dst.display())))?;
    let mut out = BufWriter::new(out);
    let mut comp = Vec::new();

    for (off, cl, rl) in table {
        f.seek(SeekFrom::Start(off))
            .map_err(|e| SnapError::new(format!("对象读取失败: {e}")))?;
        comp.resize(cl, 0);
        f.read_exact(&mut comp)
            .map_err(|e| SnapError::new(format!("对象数据损坏: {e}")))?;
        let mut dec = ZlibDecoder::new(&comp[..]);
        let mut raw = Vec::with_capacity(rl);
        dec.read_to_end(&mut raw)
            .map_err(|e| SnapError::new(format!("对象解压失败: {e}")))?;
        if raw.len() != rl {
            return Err(SnapError::new(format!(
                "对象内容长度不对（期望 {rl}，实际 {}）",
                raw.len()
            )));
        }
        out.write_all(&raw)
            .map_err(|e| SnapError::new(format!("无法写入 {}: {e}", dst.display())))?;
    }
    out.flush()
        .map_err(|e| SnapError::new(format!("无法写入 {}: {e}", dst.display())))?;
    Ok(())
}
