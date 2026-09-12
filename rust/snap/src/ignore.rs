//! .snapignore 规则：编译与匹配。
//!
//! 语义与 Python 版完全一致：
//!   *.log        不含 "/" 的模式，匹配任意一层里的同名条目（支持 fnmatch 的 [] 字符类）
//!   logs/        以 "/" 结尾是目录规则，只对目录本身生效
//!   keep/c.txt   含 "/" 的模式整体匹配相对路径，"*" 不跨 "/"，"**" 跨任意层级
//!   # 开头的行和空行忽略；不支持 "!" 反选

/// 一条规则：模式文本 + 是否目录规则。
#[derive(Clone, Debug)]
pub struct Pattern {
    pub pat: String,
    pub dir_only: bool,
}

/// 默认忽略（与 Python 版的 DEFAULT_IGNORE 一致）。
pub const DEFAULT_IGNORE: [&str; 9] = [
    ".snap/",
    ".vsnap/",
    ".git/",
    "__pycache__/",
    "*.pyc",
    "*.pyo",
    ".DS_Store",
    "*.swp",
    "*~",
];

/// 把规则文本编译成规则列表，顺序即优先级。
pub fn compile<'a, I: IntoIterator<Item = &'a str>>(lines: I) -> Vec<Pattern> {
    let mut out = Vec::new();
    for raw in lines {
        let pat = raw.trim();
        if pat.is_empty() || pat.starts_with('#') {
            continue;
        }
        let dir_only = pat.ends_with('/');
        let pat = pat.trim_end_matches('/');
        if !pat.is_empty() {
            out.push(Pattern {
                pat: pat.to_string(),
                dir_only,
            });
        }
    }
    out
}

/// rel 必须是正斜杠分隔的相对路径。
pub fn is_ignored(rel: &str, pats: &[Pattern], is_dir: bool) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    for p in pats {
        if p.pat.contains('/') {
            // 含 "/" 的模式整体匹配；目录规则只对目录本身生效
            if glob_match(&p.pat, rel) && (is_dir || !p.dir_only) {
                return true;
            }
            continue;
        }
        // 不含 "/" 的模式匹配任意一层
        let segs: &[&str] = if p.dir_only {
            if is_dir {
                &parts[..]
            } else {
                &parts[..parts.len().saturating_sub(1)]
            }
        } else {
            &parts[..]
        };
        if segs.iter().any(|s| segment_match(s, &p.pat)) {
            return true;
        }
    }
    false
}

/// 含 "/" 的模式：整路径匹配，"*" 不跨 "/"，"**" 跨任意层级。
fn glob_match(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    glob_rec(&p, &t)
}

fn glob_rec(p: &[char], s: &[char]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    if p[0] == '*' && p.len() >= 2 && p[1] == '*' {
        let rest = &p[2..];
        if rest.first() == Some(&'/') {
            // "**/" 可以是零层或多层
            let after = &rest[1..];
            if glob_rec(after, s) {
                return true;
            }
            for i in 0..s.len() {
                if s[i] == '/' && glob_rec(after, &s[i + 1..]) {
                    return true;
                }
            }
            return false;
        }
        // "**" 跨任意层级
        for i in 0..=s.len() {
            if glob_rec(rest, &s[i..]) {
                return true;
            }
        }
        return false;
    }
    if p[0] == '*' {
        let rest = &p[1..];
        let mut i = 0;
        loop {
            if glob_rec(rest, &s[i..]) {
                return true;
            }
            if i >= s.len() || s[i] == '/' {
                return false;
            }
            i += 1;
        }
    }
    if p[0] == '?' {
        return !s.is_empty() && s[0] != '/' && glob_rec(&p[1..], &s[1..]);
    }
    if s.first() == Some(&p[0]) {
        return glob_rec(&p[1..], &s[1..]);
    }
    false
}

/// 不含 "/" 的模式：对单个路径分量做 fnmatch 语义匹配（大小写敏感，支持 [] 字符类）。
fn segment_match(seg: &str, pat: &str) -> bool {
    let s: Vec<char> = seg.chars().collect();
    let p: Vec<char> = pat.chars().collect();
    seg_rec(&s, &p)
}

fn seg_rec(s: &[char], p: &[char]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    match p[0] {
        '*' => {
            for i in 0..=s.len() {
                if seg_rec(&s[i..], &p[1..]) {
                    return true;
                }
            }
            false
        }
        '?' => !s.is_empty() && seg_rec(&s[1..], &p[1..]),
        '[' => match parse_class(p) {
            Some((set, used)) => !s.is_empty() && set.hit(s[0]) && seg_rec(&s[1..], &p[used..]),
            None => !s.is_empty() && s[0] == '[' && seg_rec(&s[1..], &p[1..]),
        },
        c => !s.is_empty() && s[0] == c && seg_rec(&s[1..], &p[1..]),
    }
}

struct CharSet {
    negate: bool,
    ranges: Vec<(char, char)>,
}

impl CharSet {
    fn hit(&self, c: char) -> bool {
        let inside = self.ranges.iter().any(|&(a, b)| c >= a && c <= b);
        inside != self.negate
    }
}

/// 解析 "[...]"；返回 (集合, 消耗的字符数)。下一字符是 ']' 时按字面处理。
fn parse_class(p: &[char]) -> Option<(CharSet, usize)> {
    let mut i = 1;
    let mut negate = false;
    if p.get(i) == Some(&'!') {
        negate = true;
        i += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    loop {
        let c = *p.get(i)?;
        if c == ']' && !first {
            return Some((CharSet { negate, ranges }, i + 1));
        }
        first = false;
        // a-z 范围
        if p.get(i + 1) == Some(&'-') && p.get(i + 2).is_some_and(|&c| c != ']') {
            ranges.push((c, p[i + 2]));
            i += 3;
        } else {
            ranges.push((c, c));
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pats(list: &[&str]) -> Vec<Pattern> {
        compile(list.iter().copied())
    }

    #[test]
    fn slash_patterns() {
        let p = pats(&["logs/", "*.log", "keep/c.txt"]);
        assert!(is_ignored("logs/debug.log", &p, false));
        assert!(is_ignored("logs", &p, true));
        assert!(!is_ignored("logs", &p, false), "目录规则不该命中同名文件");
        assert!(is_ignored("a.log", &p, false));
        assert!(is_ignored("x/y/deep.log", &p, false));
        assert!(is_ignored("keep/c.txt", &p, false));
    }

    #[test]
    fn anchored_and_globstar() {
        let p = pats(&["keep/*.log"]);
        assert!(is_ignored("keep/b.log", &p, false));
        assert!(!is_ignored("keep/nested/a.log", &p, false), "* 不该跨目录");
        let p = pats(&["keep/**/*.log"]);
        assert!(is_ignored("keep/nested/a.log", &p, false));
        assert!(is_ignored("keep/a.log", &p, false), "**/ 可以是零层");
        let p = pats(&["**/logs"]);
        assert!(is_ignored("deep/logs", &p, true));
    }

    #[test]
    fn bracket_class_in_segment() {
        let p = pats(&["*.[oa]"]);
        assert!(is_ignored("foo.o", &p, false));
        assert!(is_ignored("foo.a", &p, false));
        assert!(!is_ignored("foo.c", &p, false));
        let p = pats(&["[!a]bc"]);
        assert!(is_ignored("xbc", &p, false));
        assert!(!is_ignored("abc", &p, false));
    }

    #[test]
    fn comments_and_blank_lines() {
        let p = compile(vec!["", "  ", "# 注释", "*.tmp"].into_iter());
        assert_eq!(p.len(), 1);
        assert!(is_ignored("x.tmp", &p, false));
    }
}
