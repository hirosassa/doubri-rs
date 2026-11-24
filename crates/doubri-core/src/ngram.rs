/// Returns an iterator that yields Unicode character-level n-grams.
///
/// Each n-gram is returned as a `&str` slice of the original string.
/// For ASCII-only text, a fast path avoids Unicode character boundary computation.
pub fn char_ngrams(text: &str, n: usize) -> impl Iterator<Item = &str> {
    if n == 0 || text.len() < n {
        CharNgrams::Empty
    } else if text.is_ascii() {
        CharNgrams::Ascii(AsciiNgramIter {
            text,
            n,
            pos: 0,
            len: text.len(),
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
}

struct AsciiNgramIter<'a> {
    text: &'a str,
    n: usize,
    pos: usize,
    len: usize,
}

impl<'a> Iterator for AsciiNgramIter<'a> {
    type Item = &'a str;

    #[inline]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "hot path: `pos + n <= len` is checked before `pos` grows, and `len` is a \
                  slice length (< isize::MAX), so neither sum can overflow"
    )]
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos + self.n > self.len {
            return None;
        }
        let start = self.pos;
        self.pos += 1;
        // ASCII text has a char boundary at every byte offset, so `get` succeeds for every
        // window passing the check above; a failure could only end the iteration early.
        self.text.get(start..start + self.n)
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
}
