//! Faithful port of Python difflib.SequenceMatcher(None, a, b).ratio()
//! including the autojunk heuristic — used by the dedup gate (#49).

use std::collections::HashMap;

struct Matcher<'a> {
    a: &'a [char],
    b: &'a [char],
    b2j: HashMap<char, Vec<usize>>,
}

impl<'a> Matcher<'a> {
    fn new(a: &'a [char], b: &'a [char]) -> Self {
        let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
        for (i, &c) in b.iter().enumerate() {
            b2j.entry(c).or_default().push(i);
        }
        // autojunk: for len(b) >= 200, drop elements appearing in > 1% of b
        if b.len() >= 200 {
            let n = b.len();
            b2j.retain(|_, v| v.len() * 100 <= n);
        }
        Matcher { a, b, b2j }
    }

    fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let mut besti = alo;
        let mut bestj = blo;
        let mut bestsize = 0usize;
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.b2j.get(&self.a[i]) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j2len.get(&(j.wrapping_sub(1))).copied().unwrap_or(0) + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        // isjunk=None -> bjunk empty; extend over equal neighbours on both sides
        while besti > alo && bestj > blo && self.a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && self.a[besti + bestsize] == self.b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    fn matching_blocks_total(&self) -> usize {
        let la = self.a.len();
        let lb = self.b.len();
        let mut queue: Vec<(usize, usize, usize, usize)> = vec![(0, la, 0, lb)];
        let mut total = 0usize;
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                total += k;
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        total
    }

    fn ratio(&self) -> f64 {
        let la = self.a.len();
        let lb = self.b.len();
        if la + lb == 0 {
            return 1.0;
        }
        let m = self.matching_blocks_total();
        2.0 * m as f64 / (la + lb) as f64
    }
}

/// Python difflib.SequenceMatcher(None, a, b).ratio()
pub fn ratio(a: &str, b: &str) -> f64 {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    Matcher::new(&av, &bv).ratio()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical() {
        assert!((ratio("hello world", "hello world") - 1.0).abs() < 1e-12);
    }

    #[test]
    fn known_value() {
        // difflib.SequenceMatcher(None, "abcd", "bcde").ratio() == 0.75
        assert!((ratio("abcd", "bcde") - 0.75).abs() < 1e-12);
    }

    #[test]
    fn empty() {
        assert!((ratio("", "") - 1.0).abs() < 1e-12);
        assert_eq!(ratio("abc", ""), 0.0);
    }

    #[test]
    fn private_facts() {
        // spot check against Python semantics: partial overlap
        let r = ratio("the quick brown fox", "the quick brown dog");
        assert!(r > 0.8 && r < 1.0);
    }
}