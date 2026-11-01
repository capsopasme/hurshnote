//! 快速打开用的模糊匹配。
//!
//! 查询按空白拆成若干词，每个词都必须作为子序列（不区分大小写）出现在候选中。
//! 打分偏好：文件名中的匹配、连续匹配、单词开头（路径分隔符、`_ - .`、空格、驼峰）的匹配；
//! 跳过的字符越多分越低。

/// 候选的匹配分数；不匹配返回 None。分数越高越好。
pub fn score(query: &str, candidate: &str) -> Option<i64> {
    let tokens: Vec<&str> = query.split_whitespace().collect();
    if tokens.is_empty() {
        return Some(0);
    }
    let cand: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = cand.iter().map(|c| fold(*c)).collect();
    let name_start = cand
        .iter()
        .rposition(|&c| c == '/' || c == '\\')
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut total = 0i64;
    for t in tokens {
        let q: Vec<char> = t.chars().map(fold).collect();
        // 优先在文件名内匹配
        let s = match match_from(&q, &cand, &lower, name_start) {
            Some(s) => s + 100,
            None => match_from(&q, &cand, &lower, 0)?,
        };
        total += s;
    }
    // 短路径略优先
    Some(total - cand.len() as i64 / 8)
}

fn fold(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

fn is_boundary(cand: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let p = cand[i - 1];
    let c = cand[i];
    matches!(p, '/' | '\\' | '_' | '-' | '.' | ' ') || (p.is_lowercase() && c.is_uppercase())
}

/// 从 `start` 开始贪心匹配子序列并打分。
fn match_from(q: &[char], cand: &[char], lower: &[char], start: usize) -> Option<i64> {
    let mut score = 0i64;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    let mut i = start;
    while qi < q.len() && i < lower.len() {
        if lower[i] == q[qi] {
            score += 16;
            if is_boundary(cand, i) {
                score += 20;
            }
            match last {
                Some(l) if l + 1 == i => score += 25,
                Some(l) => score -= (3 * (i - l - 1) as i64).min(30),
                None => score -= ((i - start) as i64).min(10),
            }
            if i == start {
                score += 20;
            }
            last = Some(i);
            qi += 1;
        }
        i += 1;
    }
    (qi == q.len()).then_some(score)
}

/// 对候选排序，返回前 `limit` 个 (下标, 分数)，分数相同时保持原顺序。
pub fn rank<S: AsRef<str>>(query: &str, candidates: &[S], limit: usize) -> Vec<(usize, i64)> {
    let mut v: Vec<(usize, i64)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| score(query, c.as_ref()).map(|s| (i, s)))
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(limit);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_required() {
        assert!(score("abc", "a_b_c").is_some());
        assert!(score("abc", "acb").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn case_insensitive_and_unicode() {
        assert!(score("README", "docs/readme.md").is_some());
        assert!(score("文档", "项目/文档说明.txt").is_some());
    }

    #[test]
    fn filename_preferred() {
        let a = score("main", "src/main.rs").unwrap();
        let b = score("main", "main_tools/x.rs").unwrap();
        assert!(a > b, "{a} {b}");
    }

    #[test]
    fn consecutive_and_boundary_preferred() {
        let a = score("app", "src/app.rs").unwrap();
        let b = score("app", "src/a_p_p.rs").unwrap();
        assert!(a > b);
        let c = score("fb", "src/FooBar.rs").unwrap();
        let d = score("fb", "src/fxxxb.rs").unwrap();
        assert!(c > d);
    }

    #[test]
    fn tokens_all_required() {
        assert!(score("src main", "src/main.rs").is_some());
        assert!(score("src zzz", "src/main.rs").is_none());
    }

    #[test]
    fn ranking() {
        let c = ["tests/main_test.rs", "src/main.rs", "README.md"];
        let r = rank("main", &c, 10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, 1);
    }
}
