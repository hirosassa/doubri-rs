/// Returns an iterator that yields Unicode character-level n-grams.
///
/// Each n-gram is returned as a `&str` slice of the original string.
/// For ASCII-only text, a fast path avoids Unicode character boundary computation.
pub fn char_ngrams(text: &str, n: usize) -> impl Iterator<Item = &str> {
    if n == 0 || text.len() < n {
        CharNgrams::Empty
    } else if text.is_ascii() {
        // Contract for `AsciiNgramIter`: `1 <= n <= text.len()` (checked above), so
        // `last = len - n` cannot underflow (`saturating_sub` is only a belt-and-braces
        // spelling), and every window start `s <= last` gives `s + n <= len` and
        // `s + 1 <= len`: none of the offset sums in the iterator can overflow `usize`.
        let last = text.len().saturating_sub(n);
        CharNgrams::Ascii(AsciiNgramIter {
            text,
            n,
            pos: 0,
            last,
        })
    } else {
        let char_indices: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let num_chars = char_indices.len();
        CharNgrams::Unicode(UnicodeNgramIter {
            text,
            n,
            char_indices,
            text_len: text.len(),
            num_chars,
            pos: 0,
        })
    }
}

enum CharNgrams<'a> {
    Empty,
    Ascii(AsciiNgramIter<'a>),
    Unicode(UnicodeNgramIter<'a>),
}

impl<'a> Iterator for CharNgrams<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            CharNgrams::Empty => None,
            CharNgrams::Ascii(iter) => iter.next(),
            CharNgrams::Unicode(iter) => iter.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            CharNgrams::Empty => (0, Some(0)),
            CharNgrams::Ascii(iter) => iter.size_hint(),
            CharNgrams::Unicode(iter) => iter.size_hint(),
        }
    }

    fn count(self) -> usize {
        self.size_hint().0
    }

    // `collect` and `for_each` go through `fold`; dispatching on the variant once
    // (instead of on every `next`) keeps the per-n-gram loop tight.
    fn fold<B, F>(self, init: B, f: F) -> B
    where
        F: FnMut(B, Self::Item) -> B,
    {
        match self {
            CharNgrams::Empty => init,
            CharNgrams::Ascii(iter) => iter.fold(init, f),
            CharNgrams::Unicode(iter) => iter.fold(init, f),
        }
    }
}

/// Invariants, established once by `char_ngrams` and never broken afterwards:
/// `text` is ASCII, `1 <= n <= text.len()`, `last == text.len() - n`, `0 <= pos <= last + 1`.
/// They imply `pos + n <= text.len()` while `pos <= last`, which is why the hot loop may use
/// `wrapping_add` (equal to `+` here, without the release-mode overflow check).
struct AsciiNgramIter<'a> {
    text: &'a str,
    n: usize,
    pos: usize,
    /// Largest valid window start: `text.len() - n`.
    last: usize,
}

impl<'a> Iterator for AsciiNgramIter<'a> {
    type Item = &'a str;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos > self.last {
            return None;
        }
        let start = self.pos;
        // Struct invariants: `start <= last = len - n`, hence `start + 1 <= len` and
        // `start + n <= len` cannot wrap; `wrapping_add` is exactly `+` and only avoids the
        // release-mode overflow check in this hot loop. ASCII text has a char boundary at
        // every offset, so `get` only fails for an out-of-range window, which cannot occur.
        self.pos = start.wrapping_add(1);
        self.text.get(start..start.wrapping_add(self.n))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // `pos <= last + 1` and `last < len`, so the sum cannot wrap; the saturating
        // subtraction only yields 0 when the iterator is exhausted.
        let remaining = self.last.wrapping_add(1).saturating_sub(self.pos);
        (remaining, Some(remaining))
    }

    fn fold<B, F>(self, init: B, mut f: F) -> B
    where
        F: FnMut(B, Self::Item) -> B,
    {
        let mut acc = init;
        // Struct invariants: `last = len - n < len`, so `last + 1` cannot wrap, and for every
        // `start <= last`, `start + n <= len`. Same window/boundary reasoning as `next`.
        for start in self.pos..self.last.wrapping_add(1) {
            match self.text.get(start..start.wrapping_add(self.n)) {
                Some(ngram) => acc = f(acc, ngram),
                None => break,
            }
        }
        acc
    }
}

struct UnicodeNgramIter<'a> {
    text: &'a str,
    n: usize,
    char_indices: Vec<usize>,
    text_len: usize,
    num_chars: usize,
    pos: usize,
}

impl<'a> Iterator for UnicodeNgramIter<'a> {
    type Item = &'a str;

    fn size_hint(&self) -> (usize, Option<usize>) {
        // Windows left: `num_chars - n + 1 - pos`, or 0 once exhausted (or if `n` exceeds
        // the text, which `char_ngrams` rules out).
        let remaining = self
            .num_chars
            .checked_sub(self.n)
            .and_then(|d| d.checked_add(1))
            .and_then(|w| w.checked_sub(self.pos))
            .unwrap_or(0);
        (remaining, Some(remaining))
    }

    fn next(&mut self) -> Option<Self::Item> {
        let end_pos = self
            .pos
            .checked_add(self.n)
            .filter(|&e| e <= self.num_chars)?;
        let start = *self.char_indices.get(self.pos)?;
        let end = if end_pos < self.num_chars {
            *self.char_indices.get(end_pos)?
        } else {
            self.text_len
        };
        self.pos = self.pos.checked_add(1)?;
        self.text.get(start..end)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::string_slice,
    clippy::format_push_string,
    clippy::format_collect,
    clippy::naive_bytecount,
    reason = "test code"
)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii_bigrams() {
        let result: Vec<&str> = char_ngrams("abcd", 2).collect();
        assert_eq!(result, vec!["ab", "bc", "cd"]);
    }

    #[test]
    fn test_ascii_trigrams() {
        let result: Vec<&str> = char_ngrams("abcde", 3).collect();
        assert_eq!(result, vec!["abc", "bcd", "cde"]);
    }

    #[test]
    fn test_5gram_default() {
        let result: Vec<&str> = char_ngrams("hello world", 5).collect();
        assert_eq!(
            result,
            vec![
                "hello", "ello ", "llo w", "lo wo", "o wor", " worl", "world"
            ]
        );
    }

    #[test]
    fn test_cjk_characters() {
        let result: Vec<&str> = char_ngrams("こんにちは世界", 3).collect();
        assert_eq!(
            result,
            vec!["こんに", "んにち", "にちは", "ちは世", "は世界"]
        );
    }

    #[test]
    fn test_emoji() {
        let result: Vec<&str> = char_ngrams("🎉🎊🎈🎁", 2).collect();
        assert_eq!(result, vec!["🎉🎊", "🎊🎈", "🎈🎁"]);
    }

    #[test]
    fn test_text_shorter_than_n() {
        let result: Vec<&str> = char_ngrams("ab", 5).collect();
        assert_eq!(result, Vec::<&str>::new());
    }

    #[test]
    fn test_text_equal_to_n() {
        let result: Vec<&str> = char_ngrams("abc", 3).collect();
        assert_eq!(result, vec!["abc"]);
    }

    #[test]
    fn test_empty_string() {
        let result: Vec<&str> = char_ngrams("", 5).collect();
        assert_eq!(result, Vec::<&str>::new());
    }

    #[test]
    fn test_n_is_1() {
        let result: Vec<&str> = char_ngrams("abc", 1).collect();
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_n_is_0() {
        let result: Vec<&str> = char_ngrams("abc", 0).collect();
        assert_eq!(result, Vec::<&str>::new());
    }

    #[test]
    fn test_mixed_ascii_and_unicode() {
        let result: Vec<&str> = char_ngrams("aあb", 2).collect();
        assert_eq!(result, vec!["aあ", "あb"]);
    }

    #[test]
    fn test_unicode_text_exactly_ngram_size_yields_one_ngram() {
        let result: Vec<&str> = char_ngrams("あいうえお", 5).collect();
        assert_eq!(result, vec!["あいうえお"]);
    }

    #[test]
    fn test_size_hint_is_exact_before_and_during_iteration() {
        for text in ["", "ab", "abcdef", "héllo wörld", "日本語のテキスト"] {
            for n in 0..=text.chars().count() + 1 {
                let mut iter = char_ngrams(text, n);
                // A `for` loop drives `next`, independent of `size_hint`/`count`.
                let mut expected = 0usize;
                for _ in char_ngrams(text, n) {
                    expected += 1;
                }
                assert_eq!(
                    iter.size_hint(),
                    (expected, Some(expected)),
                    "{text:?} n={n}"
                );
                if iter.next().is_some() {
                    let left = expected - 1;
                    assert_eq!(
                        iter.size_hint(),
                        (left, Some(left)),
                        "{text:?} n={n} after next"
                    );
                }
            }
        }
    }

    #[test]
    fn test_count_and_collect_agree_with_manual_iteration() {
        for text in ["", "abcdefgh", "ab", "héllo wörld", "日本語のテキスト"] {
            for n in 1..=4 {
                let mut manual = Vec::new();
                for g in char_ngrams(text, n) {
                    manual.push(g);
                }
                assert_eq!(char_ngrams(text, n).count(), manual.len(), "{text:?} n={n}");
                assert_eq!(
                    char_ngrams(text, n).collect::<Vec<_>>(),
                    manual,
                    "{text:?} n={n}"
                );
            }
        }
    }

    #[test]
    fn test_ascii_ngrams_are_exact_windows() {
        assert_eq!(
            char_ngrams("abcde", 3).collect::<Vec<_>>(),
            vec!["abc", "bcd", "cde"]
        );
        assert_eq!(char_ngrams("abc", 3).collect::<Vec<_>>(), vec!["abc"]);
        assert_eq!(char_ngrams("ab", 3).count(), 0);
    }
}
