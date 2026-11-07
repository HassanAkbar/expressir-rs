//! Remark extraction over raw EXPRESS source (issue #20, part 1).
//!
//! Byte-level port of the Ruby `Expressir::Express::RemarkScanner`
//! (ISO 10303-11 §7.1.6): a single-pass state machine that finds tail
//! (`--`) and embedded (`(* ... *)`) remarks, tracks nesting so an
//! inline `--` inside an embedded block is not mistaken for a tail,
//! skips string literals (`''` is an escaped quote), and accepts the
//! `(*)` overlap pattern as a 3-byte empty embedded remark.
//!
//! The scanner knows nothing about the model; attachment is the
//! attacher's job (part 2). Offsets and semantics match the Ruby
//! scanner byte for byte — including the Ruby `(*)` quirk where the
//! emitted `end_position` runs one byte past the pattern.

/// Tail remark (`-- ...`).
pub const FORMAT_TAIL: &str = "tail";

/// Embedded remark (`(* ... *)`).
pub const FORMAT_EMBEDDED: &str = "embedded";

/// Tail annotation (`--- ...`), the EXPRESSdoc display channel
/// (metanorma/annotated-express#94). Recognized only in annotations
/// mode, and only for exactly three dashes not followed by a fourth —
/// dash runs stay remarks.
pub const FORMAT_TAIL_ANNOTATION: &str = "tail_annotation";

/// Embedded annotation (`(** ... *)`), annotations mode only.
pub const FORMAT_EMBEDDED_ANNOTATION: &str = "embedded_annotation";

const TAIL_MARKER: &[u8] = b"--";
const EMBEDDED_OPEN: &[u8] = b"(*";
const EMBEDDED_CLOSE: &[u8] = b"*)";
const TAIL_ANNOTATION_MARKER: &[u8] = b"---";
const EMBEDDED_ANNOTATION_OPEN: &[u8] = b"(**";
const STRING_QUOTE: u8 = b'\'';
const RPAREN_BYTE: u8 = b')';

/// One extracted remark, in the shape of the Ruby
/// `RemarkScanner::Remark` struct.
///
/// `position` is the byte offset of the opener (`--` or `(*`);
/// `end_position` is the byte offset one past the closer (the newline
/// byte for tail remarks, the byte after `*)` for embedded remarks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemarkUnit {
    pub position: usize,
    pub end_position: usize,
    pub line: usize,
    pub text: String,
    pub tag: Option<String>,
    pub format: &'static str,
}

impl RemarkUnit {
    pub fn tail(&self) -> bool {
        self.format == FORMAT_TAIL
    }

    pub fn embedded(&self) -> bool {
        self.format == FORMAT_EMBEDDED
    }

    /// Tail placement semantics — annotations sit where tails sit.
    pub fn tail_like(&self) -> bool {
        self.format == FORMAT_TAIL || self.format == FORMAT_TAIL_ANNOTATION
    }

    pub fn tail_annotation(&self) -> bool {
        self.format == FORMAT_TAIL_ANNOTATION
    }

    pub fn embedded_annotation(&self) -> bool {
        self.format == FORMAT_EMBEDDED_ANNOTATION
    }

    /// Byte length of the full remark (opener + content + closer).
    pub fn bytesize(&self) -> usize {
        self.end_position - self.position
    }
}

/// Line-start offset table mirroring the Ruby LineMap: `offsets[0] =
/// 0`, `offsets[i]` = the i-th newline's position + 1. The 1-based
/// line containing `pos` is the count of offsets `<= pos`.
struct LineMap {
    offsets: Vec<usize>,
}

impl LineMap {
    fn new(source: &[u8]) -> Self {
        let mut offsets = vec![0usize];
        for (i, b) in source.iter().enumerate() {
            if *b == b'\n' {
                offsets.push(i + 1);
            }
        }
        LineMap { offsets }
    }

    fn line_number(&self, byte_pos: usize) -> usize {
        self.offsets.partition_point(|offset| *offset <= byte_pos)
    }
}

fn find(source: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= source.len() {
        return None;
    }
    let first = needle[0];
    let last_start = source.len().checked_sub(needle.len())?;
    let mut i = from;
    while i <= last_start {
        let offset = source[i..=last_start].iter().position(|b| *b == first)?;
        let candidate = i + offset;
        if &source[candidate..candidate + needle.len()] == needle {
            return Some(candidate);
        }
        i = candidate + 1;
    }
    None
}

/// A forward-only memo for one needle: queries are monotonic in `from`,
/// so each occurrence is found once and re-found only when the scan has
/// advanced past it — total work stays linear even for sources dense
/// with one marker and sparse in another.
struct Cursor<'a> {
    src: &'a [u8],
    needle: &'a [u8],
    next: Option<usize>,
    searched_from: usize,
}

impl<'a> Cursor<'a> {
    fn new(src: &'a [u8], needle: &'a [u8]) -> Self {
        Cursor {
            src,
            needle,
            next: None,
            searched_from: 0,
        }
    }

    fn at_or_after(&mut self, from: usize) -> Option<usize> {
        if from < self.searched_from {
            // Defensive: callers are monotonic, but never return a stale
            // position if that assumption breaks.
            self.next = None;
            self.searched_from = 0;
        }
        if let Some(next) = self.next {
            if next >= from {
                return Some(next);
            }
        } else if from >= self.searched_from && self.searched_from > 0 {
            return None;
        }
        self.searched_from = from;
        self.next = find(self.src, self.needle, from);
        self.next
    }
}

/// Ruby `String#strip`: only ASCII whitespace (plus NUL) is trimmed —
/// never Unicode whitespace.
fn ruby_strip(bytes: &[u8]) -> &[u8] {
    let is_ws = |b: u8| matches!(b, b'\0' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ');
    let start = bytes.iter().position(|b| !is_ws(*b)).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !is_ws(*b))
        .map_or(start, |i| i + 1);
    &bytes[start..end]
}

fn to_string(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec())
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// The whitespace set the Ruby remark regexes' `\s` matches
/// (`[ \t\r\n\f\v]`).
fn is_regex_space(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ')
}

/// ISO 10303-11 §7.1.6.3 remark tags. Tail remarks accept both the
/// quoted form and the `IP<n>:` informal-proposition shorthand
/// (historic behaviour: IP tags are tail-only); embedded remarks take
/// the quoted form only. Returns `(tag, content)`; untagged input
/// yields a nil tag and the text as-is.
fn parse_tail(text: &[u8]) -> (Option<String>, String) {
    parse_ip_tag(text)
        .or_else(|| parse_quoted_tag(text))
        .unwrap_or_else(|| (None, to_string(text)))
}

fn parse_embedded(content: &[u8]) -> (Option<String>, String) {
    let stripped = ruby_strip(content);
    parse_quoted_tag(stripped).unwrap_or_else(|| (None, to_string(stripped)))
}

/// `\A(IP\d+):\s*(.*)\z` — greedy `\s*` consumes all whitespace after
/// the colon; `(.*)` takes the remainder verbatim (possibly empty).
fn parse_ip_tag(text: &[u8]) -> Option<(Option<String>, String)> {
    let rest = text.strip_prefix(b"IP")?;
    let digits_end = rest.iter().position(|b| !b.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    let tag = format!("IP{}", to_string(&rest[..digits_end]));
    let after = rest[digits_end..].strip_prefix(b":")?;
    let content = match after.iter().position(|b| !is_regex_space(*b)) {
        Some(start) => to_string(&after[start..]),
        None => String::new(),
    };
    Some((Some(tag), content))
}

/// `\A"([^"]*)"\s*(.*)\z` — the tag runs to the next quote (no
/// escapes); whitespace after the quote is consumed, the remainder is
/// the content (possibly empty).
fn parse_quoted_tag(text: &[u8]) -> Option<(Option<String>, String)> {
    let body = text.strip_prefix(b"\"")?;
    let close = body.iter().position(|b| *b == b'"')?;
    let tag = to_string(&body[..close]);
    let after = &body[close + 1..];
    let content = match after.iter().position(|b| !is_regex_space(*b)) {
        Some(start) => to_string(&after[start..]),
        None => String::new(),
    };
    Some((Some(tag), content))
}

/// True when bytes at `pos` form the `(*)` overlap: opener `(*` and
/// closer `*)` share the `*` (issue #126).
fn empty_overlap_remark(src: &[u8], pos: usize) -> bool {
    pos + 2 < src.len() && src[pos + 2] == RPAREN_BYTE
}

/// Extracts every remark from `source`, in source order (by byte
/// position). Unclosed remarks and strings terminate the scan
/// gracefully — malformed input never raises.
pub fn scan(source: &str) -> Vec<RemarkUnit> {
    scan_with(source, false)
}

/// True when bytes at `pos` open a `---` tail annotation: exactly
/// three dashes, the fourth byte (if any) not a dash — dash runs of
/// four or more stay tail remarks.
fn tail_annotation_at(src: &[u8], pos: usize) -> bool {
    src[pos..].starts_with(TAIL_ANNOTATION_MARKER)
        && src.get(pos + TAIL_ANNOTATION_MARKER.len()) != Some(&b'-')
}

/// True when bytes at `pos` open a `(**` embedded annotation.
fn embedded_annotation_at(src: &[u8], pos: usize) -> bool {
    src[pos..].starts_with(EMBEDDED_ANNOTATION_OPEN)
}

/// Extracts every remark (and, in annotations mode, every EXPRESSdoc
/// annotation — metanorma/annotated-express#94) from `source`, in
/// source order (by byte position). Unclosed remarks and strings
/// terminate the scan gracefully — malformed input never raises.
pub fn scan_with(source: &str, annotations: bool) -> Vec<RemarkUnit> {
    let src = source.as_bytes();
    let line_map = LineMap::new(src);
    let mut remarks = Vec::new();

    #[derive(PartialEq)]
    enum State {
        Top,
        InTail,
        InEmbedded,
        InString,
    }

    let mut state = State::Top;
    let mut pos = 0usize;
    let mut embedded_depth = 0usize;
    let mut tail_start = 0usize;
    let mut tail_line = 1usize;
    let mut tail_ann = false;
    let mut embedded_start = 0usize;
    let mut embedded_line = 1usize;
    let mut embedded_ann = false;
    let mut embed = Cursor::new(src, EMBEDDED_OPEN);
    let mut close = Cursor::new(src, EMBEDDED_CLOSE);
    let mut tail = Cursor::new(src, TAIL_MARKER);
    let mut quote = Cursor::new(src, b"'");
    let mut newline = Cursor::new(src, b"\n");

    while pos < src.len() {
        match state {
            State::InTail => match newline.at_or_after(pos) {
                Some(nl) => {
                    emit_tail(&mut remarks, src, tail_start, nl, tail_line, tail_ann);
                    pos = nl + 1;
                    state = State::Top;
                }
                None => {
                    emit_tail(
                        &mut remarks,
                        src,
                        tail_start,
                        src.len(),
                        tail_line,
                        tail_ann,
                    );
                    return remarks;
                }
            },
            State::InEmbedded => {
                let closing = close.at_or_after(pos);
                let nesting = embed.at_or_after(pos);
                if let Some(n) = nesting {
                    if closing.is_none_or(|c| n < c) {
                        if empty_overlap_remark(src, n) {
                            // `(*)` opens and closes immediately; depth
                            // unchanged.
                            pos = n + 3;
                        } else {
                            embedded_depth += 1;
                            pos = n + EMBEDDED_OPEN.len();
                        }
                        continue;
                    }
                }
                match closing {
                    Some(c) => {
                        embedded_depth -= 1;
                        pos = c + EMBEDDED_CLOSE.len();
                        if embedded_depth == 0 {
                            emit_embedded(
                                &mut remarks,
                                src,
                                embedded_start,
                                c,
                                embedded_line,
                                embedded_ann,
                            );
                            state = State::Top;
                        }
                    }
                    // Unclosed embedded remark — terminate gracefully.
                    None => return remarks,
                }
            }
            State::InString => {
                // `''` inside a string is an escaped quote (§7.1.6.5).
                let Some(q) = quote.at_or_after(pos) else {
                    return remarks;
                };
                if q + 1 < src.len() && src[q + 1] == STRING_QUOTE {
                    pos = q + 2;
                } else {
                    pos = q + 1;
                    state = State::Top;
                }
            }
            State::Top => {
                let next_embed = embed.at_or_after(pos);
                let next_tail = tail.at_or_after(pos);
                let next_quote = quote.at_or_after(pos);
                if next_embed.is_none() && next_tail.is_none() && next_quote.is_none() {
                    return remarks;
                }
                let quote_first = next_quote.is_some()
                    && next_embed.is_none_or(|e| next_quote.unwrap() < e)
                    && next_tail.is_none_or(|t| next_quote.unwrap() < t);
                let embed_first =
                    next_embed.is_some() && next_tail.is_none_or(|t| next_embed.unwrap() <= t);
                if quote_first {
                    pos = next_quote.unwrap() + 1;
                    state = State::InString;
                } else if embed_first {
                    let n = next_embed.unwrap();
                    if empty_overlap_remark(src, n) {
                        // Standalone `(*)`: empty embedded remark. The Ruby
                        // scanner emits end_position = n + 4 — one byte
                        // past the pattern; parity keeps the quirk.
                        emit_embedded(
                            &mut remarks,
                            src,
                            n + EMBEDDED_OPEN.len(),
                            n + EMBEDDED_OPEN.len(),
                            line_map.line_number(n),
                            false,
                        );
                        pos = n + 3;
                    } else {
                        // `(**` opens an annotation when annotations mode
                        // is on; the byte after `(*` is a `*`, so the
                        // `(*)` overlap can never collide with it.
                        embedded_ann = annotations && embedded_annotation_at(src, n);
                        embedded_start = n + if embedded_ann {
                            EMBEDDED_ANNOTATION_OPEN.len()
                        } else {
                            EMBEDDED_OPEN.len()
                        };
                        embedded_line = line_map.line_number(n);
                        embedded_depth = 1;
                        state = State::InEmbedded;
                        pos = embedded_start;
                    }
                } else {
                    let t = next_tail.unwrap();
                    tail_ann = annotations && tail_annotation_at(src, t);
                    tail_start = t + if tail_ann {
                        TAIL_ANNOTATION_MARKER.len()
                    } else {
                        TAIL_MARKER.len()
                    };
                    tail_line = line_map.line_number(t);
                    state = State::InTail;
                    pos = tail_start;
                }
            }
        }
    }

    remarks
}

fn emit_tail(
    remarks: &mut Vec<RemarkUnit>,
    src: &[u8],
    content_start: usize,
    content_end: usize,
    line: usize,
    annotation: bool,
) {
    let raw = &src[content_start..content_end];
    let (tag, content) = parse_tail(ruby_strip(raw));
    remarks.push(RemarkUnit {
        position: content_start
            - if annotation {
                TAIL_ANNOTATION_MARKER.len()
            } else {
                TAIL_MARKER.len()
            },
        end_position: content_end,
        line,
        text: content,
        tag,
        format: if annotation {
            FORMAT_TAIL_ANNOTATION
        } else {
            FORMAT_TAIL
        },
    });
}

fn emit_embedded(
    remarks: &mut Vec<RemarkUnit>,
    src: &[u8],
    content_start: usize,
    content_end: usize,
    line: usize,
    annotation: bool,
) {
    let raw = &src[content_start..content_end];
    let (tag, content) = parse_embedded(raw);
    remarks.push(RemarkUnit {
        position: content_start
            - if annotation {
                EMBEDDED_ANNOTATION_OPEN.len()
            } else {
                EMBEDDED_OPEN.len()
            },
        end_position: content_end + EMBEDDED_CLOSE.len(),
        line,
        text: content,
        tag,
        format: if annotation {
            FORMAT_EMBEDDED_ANNOTATION
        } else {
            FORMAT_EMBEDDED
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(source: &str) -> Vec<RemarkUnit> {
        crate::remark::scan(source)
    }

    fn scan_ann(source: &str) -> Vec<RemarkUnit> {
        crate::remark::scan_with(source, true)
    }

    #[test]
    fn annotations_off_by_default_keeps_dash_runs_as_remarks() {
        let source = "--- a three dash remark\n-- a plain remark\n---- four dashes\n(** looks like a remark opener *)\n";
        let units = scan(source);
        assert_eq!(units.len(), 4);
        assert!(units
            .iter()
            .all(|u| u.format == FORMAT_TAIL || u.format == FORMAT_EMBEDDED));
    }

    #[test]
    fn annotations_mode_classifies_the_expressdoc_forms() {
        let source = concat!(
            "SCHEMA demo; --- schema annotation\n",
            "  (**\"demo.note\" embedded annotation *)\n",
            "  --- tail annotation\n",
            "  ---- four dash divider stays a remark\n",
            "  (* a plain remark *)\n",
            "END_SCHEMA;\n",
        );
        let units = scan_ann(source);
        let formats: Vec<&str> = units.iter().map(|u| u.format).collect();
        assert_eq!(
            formats,
            vec![
                FORMAT_TAIL_ANNOTATION,
                FORMAT_EMBEDDED_ANNOTATION,
                FORMAT_TAIL_ANNOTATION,
                FORMAT_TAIL,
                FORMAT_EMBEDDED,
            ]
        );
        let schema_ann = &units[0];
        assert_eq!(schema_ann.text, "schema annotation");
        let embedded_ann = &units[1];
        assert_eq!(embedded_ann.tag.as_deref(), Some("demo.note"));
        assert_eq!(embedded_ann.text, "embedded annotation");
    }

    #[test]
    fn annotation_positions_cover_their_markers() {
        let source = "SCHEMA s;\nEND_SCHEMA;\n--- note\n";
        let marker_at = source.find("---").expect("marker present");
        let units = scan_ann(source);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].position, marker_at);
        assert_eq!(&source[units[0].position..units[0].position + 3], "---");
    }

    #[test]
    fn plain_markers_inside_annotations_nest_like_remarks() {
        // An embedded remark opener inside an annotation opens a nested
        // remark; the annotation closes on its own `*)`.
        let source = "SCHEMA s;\n  (** outer (** inner *) still annotation *)\nEND_SCHEMA;\n";
        let units = scan_ann(source);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].format, FORMAT_EMBEDDED_ANNOTATION);
        assert!(units[0].text.contains("inner"));
    }

    #[test]
    fn annotation_opener_takes_priority_over_the_plain_marker() {
        // `(**` is `(*` plus a star: the annotation wins at the same
        // position, and the following plain remark is a separate unit.
        let source = "(** ann *) (* plain *)\n";
        let units = scan_ann(source);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].format, FORMAT_EMBEDDED_ANNOTATION);
        assert_eq!(units[0].text, "ann");
        assert_eq!(units[1].format, FORMAT_EMBEDDED);
        assert_eq!(units[1].text, "plain");
    }

    #[test]
    fn inline_dash_inside_embedded_is_not_a_tail() {
        let source = concat!(
            "SCHEMA mini_schema;\n",
            "(*\n",
            "This schema provides representations for items a) -- d). that are",
            " mandatory in any application.\n",
            "\n",
            "[NOTE]\n",
            "--\n",
            "Scenarios of the expected use are described in the introduction.\n",
            "--\n",
            "*)\n",
            "END_SCHEMA;\n",
        );
        let remarks = scan(source);
        assert_eq!(remarks.len(), 1);
        assert!(remarks[0].embedded());
        assert!(remarks[0].text.contains("items a) -- d)."));
        assert!(remarks[0].text.contains("[NOTE]\n--\nScenarios"));
    }

    #[test]
    fn nested_embedded_captures_full_content() {
        let remarks = scan("(* outer (* inner *) still outer *)");
        assert_eq!(remarks.len(), 1);
        assert!(remarks[0].embedded());
        assert_eq!(remarks[0].text, "outer (* inner *) still outer");
    }

    #[test]
    fn three_levels_of_nesting() {
        let remarks = scan("(* a (* b (* c *) b *) a *)");
        assert_eq!(remarks.len(), 1);
        assert_eq!(remarks[0].text, "a (* b (* c *) b *) a");
    }

    #[test]
    fn iso_nested_example_with_quote_symbols() {
        let source = "(* The '(*' symbol starts a remark, and the '*)' symbol ends it *)";
        let remarks = scan(source);
        assert!(remarks[0].embedded());
        assert_eq!(
            remarks[0].text,
            "The '(*' symbol starts a remark, and the '*)' symbol ends it"
        );
    }

    #[test]
    fn simple_tail_remark() {
        let source = "SCHEMA x;\n-- hello world\nEND_SCHEMA;\n";
        let remarks = scan(source);
        let tail = remarks.iter().find(|r| r.tail()).expect("tail");
        assert_eq!(tail.text, "hello world");
        assert_eq!(tail.tag, None);
        assert_eq!(tail.line, 2);
    }

    #[test]
    fn tail_with_quoted_tag() {
        let remark = &scan("--\"schema_x.attr\" tagged tail remark\n")[0];
        assert_eq!(remark.tag.as_deref(), Some("schema_x.attr"));
        assert_eq!(remark.text, "tagged tail remark");
    }

    #[test]
    fn tail_with_ip_tag() {
        let remark = &scan("-- IP1: an informal proposition\n")[0];
        assert_eq!(remark.tag.as_deref(), Some("IP1"));
        assert_eq!(remark.text, "an informal proposition");
        assert!(remark.tail());
    }

    #[test]
    fn tail_trailing_whitespace_stripped() {
        let remarks = scan("-- tail with trailing spaces   \n");
        assert_eq!(remarks[0].text, "tail with trailing spaces");
    }

    #[test]
    fn tail_to_end_of_source_without_newline() {
        let remarks = scan("CODE; -- end of file remark");
        assert_eq!(remarks[0].text, "end of file remark");
        assert_eq!(
            remarks[0].end_position,
            remarks[0].position + remarks[0].bytesize()
        );
    }

    #[test]
    fn tail_after_embedded_on_same_line() {
        let remarks = scan("SCHEMA test;\n(* block *) -- tail after block\nEND_SCHEMA;\n");
        let tails: Vec<&str> = remarks
            .iter()
            .filter(|r| r.tail())
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(tails, vec!["tail after block"]);
    }

    #[test]
    fn embedded_opened_inside_tail_is_tail_content() {
        let remarks = scan("a : INTEGER; -- see (* note\nb : INTEGER; -- other\n(* real *)\n");
        let shapes: Vec<(&str, &str)> = remarks
            .iter()
            .map(|r| (r.format, r.text.as_str()))
            .collect();
        assert_eq!(
            shapes,
            vec![
                ("tail", "see (* note"),
                ("tail", "other"),
                ("embedded", "real"),
            ]
        );
    }

    #[test]
    fn single_line_embedded() {
        let remarks = scan("(* hello *)");
        assert_eq!(remarks[0].text, "hello");
    }

    #[test]
    fn multiline_embedded_keeps_inner_indent() {
        let source = "(*\n  first line\n  second line\n*)\n";
        let remarks = scan(source);
        assert_eq!(remarks[0].text, "first line\n  second line");
    }

    #[test]
    fn tagged_embedded() {
        let remark = &scan("(*\"schema.entity\" remark body *)")[0];
        assert_eq!(remark.tag.as_deref(), Some("schema.entity"));
        assert_eq!(remark.text, "remark body");
    }

    #[test]
    fn tagged_embedded_keeps_inline_dash() {
        let remark = &scan("(*\"schema.entity\" body -- with dash *)")[0];
        assert_eq!(remark.tag.as_deref(), Some("schema.entity"));
        assert_eq!(remark.text, "body -- with dash");
    }

    #[test]
    fn multiple_embedded_separated_by_code() {
        let remarks = scan("(* first *) CODE; (* second *)\n");
        let formats: Vec<&str> = remarks.iter().map(|r| r.format).collect();
        let texts: Vec<&str> = remarks.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(formats, vec!["embedded", "embedded"]);
        assert_eq!(texts, vec!["first", "second"]);
    }

    #[test]
    fn single_line_block_remark_has_no_tail() {
        let remarks = scan("SCHEMA test;\n(* prose -- more prose *)\nEND_SCHEMA;\n");
        assert_eq!(remarks[0].format, "embedded");
        assert_eq!(remarks.len(), 1);
    }

    #[test]
    fn dash_inside_string_is_not_a_tail() {
        let source = "CONSTANT greeting : STRING := 'hello -- world'; END_CONSTANT;\n";
        assert_eq!(scan(source), vec![]);
    }

    #[test]
    fn embedded_open_inside_string_is_not_a_remark() {
        let source = "CONSTANT note : STRING := 'see (* not a block *) here'; END_CONSTANT;\n";
        assert_eq!(scan(source), vec![]);
    }

    #[test]
    fn doubled_quote_escape_keeps_dash_inside_string() {
        let source = "x : STRING := 'it''s -- ok';\n";
        assert_eq!(scan(source), vec![]);
    }

    #[test]
    fn scanning_resumes_after_string_closes() {
        let source = "x : STRING := 'no -- dash'; -- real tail\n";
        let remarks = scan(source);
        assert_eq!(remarks.len(), 1);
        assert!(remarks[0].tail());
        assert_eq!(remarks[0].text, "real tail");
    }

    #[test]
    fn unclosed_string_terminates_gracefully() {
        assert_eq!(scan("x := 'never closed -- not a tail"), vec![]);
    }

    #[test]
    fn empty_source() {
        assert_eq!(scan(""), vec![]);
    }

    #[test]
    fn no_remarks() {
        assert_eq!(scan("SCHEMA x;\nEND_SCHEMA;\n"), vec![]);
    }

    #[test]
    fn unterminated_block_yields_nothing() {
        assert_eq!(
            scan("SCHEMA test;\n(* never closed\n-- not a tail\n"),
            vec![]
        );
    }

    #[test]
    fn unclosed_embedded_with_dash_inside_has_no_tails() {
        let remarks = scan("(* never closed but has -- inside");
        assert!(remarks.iter().all(|r| !r.tail()));
    }

    #[test]
    fn crlf_tail_strips_carriage_return() {
        let remarks = scan("CODE; -- tail remark\r\nnext line\n");
        assert_eq!(remarks[0].text, "tail remark");
        assert!(!remarks[0].text.contains('\r'));
    }

    #[test]
    fn crlf_embedded_keeps_crlf_content() {
        let remarks = scan("(*\r\nfirst\r\nsecond\r\n*)\r\n");
        assert_eq!(remarks[0].text, "first\r\nsecond");
    }

    #[test]
    fn overlap_pattern_alone_is_empty_embedded() {
        let remarks = scan("(*)");
        assert_eq!(remarks.len(), 1);
        assert!(remarks[0].embedded());
        assert_eq!(remarks[0].text, "");
        // The Ruby scanner emits end_position one byte past the 3-byte
        // pattern; parity keeps the quirk.
        assert_eq!(remarks[0].position, 0);
        assert_eq!(remarks[0].end_position, 4);
    }

    #[test]
    fn overlap_inside_outer_is_content() {
        let remarks = scan("(* test (*) here *)");
        assert_eq!(remarks.len(), 1);
        assert_eq!(remarks[0].text, "test (*) here");
    }

    #[test]
    fn multiple_overlaps_inside_one_remark() {
        let remarks = scan("(* before (*) after (*) end *)");
        assert_eq!(remarks.len(), 1);
        assert_eq!(remarks[0].text, "before (*) after (*) end");
    }

    #[test]
    fn overlap_inside_tagged_embedded() {
        let remarks = scan("(*\"tag\" content (*) content *)");
        assert_eq!(remarks.len(), 1);
        assert_eq!(remarks[0].tag.as_deref(), Some("tag"));
        assert_eq!(remarks[0].text, "content (*) content");
    }

    #[test]
    fn ordinary_empty_forms_still_recognised() {
        assert_eq!(scan("(* *)")[0].text, "");
        assert_eq!(scan("(**)")[0].text, "");
        assert_eq!(scan("(* a (**) c *)")[0].text, "a (**) c");
    }

    #[test]
    fn remarks_ordered_by_position() {
        let source = "-- one\n(* two *)\n-- three\n";
        let remarks = scan(source);
        let shapes: Vec<(usize, &str)> = remarks.iter().map(|r| (r.position, r.format)).collect();
        assert_eq!(shapes, vec![(0, "tail"), (7, "embedded"), (17, "tail")]);
    }

    #[test]
    fn tail_position_and_line_at_opening_marker() {
        let source = "SCHEMA test;\n-- a tail remark\nEND_SCHEMA;\n";
        let remark = &scan(source)[0];
        assert_eq!(remark.position, source.find("--").unwrap());
        assert_eq!(remark.line, 2);
        assert_eq!(remark.text, "a tail remark");
    }

    #[test]
    fn embedded_position_and_line_at_opening_marker() {
        let source = "SCHEMA test;\n\n(* doc\nspans lines *)\n-- tail\n";
        let remarks = scan(source);
        assert_eq!(remarks[0].position, source.find("(*").unwrap());
        assert_eq!(remarks[0].line, 3);
        assert_eq!(remarks[1].line, 5);
    }

    #[test]
    fn multibyte_text_preserved_with_byte_positions() {
        let source = "name : STRING; -- Name in Japanese: \u{540D}\u{524D}\n";
        let remark = &scan(source)[0];
        assert_eq!(remark.text, "Name in Japanese: \u{540D}\u{524D}");
        assert_eq!(remark.position, source.find("--").unwrap());
    }

    #[test]
    fn line_map_matches_ruby_semantics() {
        let map = LineMap::new(b"hello\nworld\n");
        assert_eq!(map.line_number(0), 1);
        assert_eq!(map.line_number(5), 1);
        assert_eq!(map.line_number(6), 2);
        assert_eq!(map.line_number(11), 2);
        assert_eq!(map.line_number(12), 3);
    }

    #[test]
    fn tag_with_no_content_after_quote_is_empty() {
        assert_eq!(scan("--\"solo\"\n")[0].tag.as_deref(), Some("solo"));
        assert_eq!(scan("--\"solo\"\n")[0].text, "");
        assert_eq!(scan("(*\"solo\"*)")[0].text, "");
    }

    #[test]
    fn ip_shorthand_requires_colon() {
        // No colon after the digits — falls through to untagged.
        let remark = &scan("-- IP1 no colon here\n")[0];
        assert_eq!(remark.tag, None);
        assert_eq!(remark.text, "IP1 no colon here");
    }
}
