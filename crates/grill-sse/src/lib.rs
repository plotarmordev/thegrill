// Bounded server-sent-event framing only: no HTTP, retry, reconnection or
// last-event-id policy. Lines end at CR, LF or CRLF (across reads); one leading
// UTF-8 BOM is dropped; every line must be strict UTF-8; `:` lines are comments;
// unknown fields are ignored; a blank line dispatches accumulated `data` lines;
// nothing is dispatched implicitly at EOF.
/// Caller-owned framing limits. No buffers are preallocated to these sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SseLimits {
    /// Maximum line bytes, excluding the CR/LF terminator.
    pub line_bytes: usize,
    /// Maximum buffered data, including the LF appended for each data field.
    /// The final appended LF is removed before invoking the handler.
    pub event_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitError {
    Line,
    Event,
}

impl std::fmt::Display for LimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let field = match self {
            Self::Line => "line_bytes",
            Self::Event => "event_bytes",
        };
        write!(f, "{field} must be in 1..=isize::MAX")
    }
}

impl std::error::Error for LimitError {}

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, PartialEq, Eq)]
pub enum Framing {
    Line,
    Event,
    Utf8,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Framing(Framing),
    Handler(E),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

pub struct Parser {
    limits: SseLimits,
    line: Vec<u8>,
    data: Vec<u8>,
    has_data: bool,
    pending_cr: bool,
    started: bool,
}

impl Parser {
    pub fn new(limits: SseLimits) -> Result<Self, LimitError> {
        if limits.line_bytes == 0 || limits.line_bytes > isize::MAX as usize {
            return Err(LimitError::Line);
        }
        if limits.event_bytes == 0 || limits.event_bytes > isize::MAX as usize {
            return Err(LimitError::Event);
        }
        Ok(Self {
            limits,
            line: Vec::new(),
            data: Vec::new(),
            has_data: false,
            pending_cr: false,
            started: false,
        })
    }

    /// Consume one received chunk. Returns `Some(consumed)` when the handler
    /// stopped after an event whose terminating line ended `consumed` bytes into
    /// this chunk; `None` when the whole chunk was consumed and reading continues.
    /// Discard the parser after a framing or handler error. Event semantics and
    /// completion markers belong to the handler, not this framing layer.
    pub fn feed<E>(
        &mut self,
        chunk: &[u8],
        mut handler: impl FnMut(&[u8]) -> Result<Flow, E>,
    ) -> Result<Option<usize>, Error<E>> {
        if chunk.is_empty() {
            return Ok(None);
        }
        let mut i = 0;
        if self.pending_cr {
            self.pending_cr = false;
            if chunk.first() == Some(&b'\n') {
                i = 1;
            }
        }
        while i < chunk.len() {
            let Some(end) = chunk[i..].iter().position(|b| *b == b'\r' || *b == b'\n') else {
                self.push(&chunk[i..])?;
                return Ok(None);
            };
            self.push(&chunk[i..i + end])?;
            let mut next = i + end + 1;
            if chunk[i + end] == b'\r' {
                match chunk.get(next) {
                    Some(b'\n') => next += 1,
                    Some(_) => {}
                    None => self.pending_cr = true,
                }
            }
            let flow = self.complete_line(&mut handler)?;
            i = next;
            if flow == Flow::Stop {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    fn push<E>(&mut self, piece: &[u8]) -> Result<(), Error<E>> {
        if piece.len() > self.limits.line_bytes - self.line.len() {
            return Err(Error::Framing(Framing::Line));
        }
        self.line.extend_from_slice(piece);
        Ok(())
    }

    fn complete_line<E>(
        &mut self,
        handler: &mut impl FnMut(&[u8]) -> Result<Flow, E>,
    ) -> Result<Flow, Error<E>> {
        let mut line = self.line.as_slice();
        if !self.started {
            self.started = true;
            if let Some(rest) = line.strip_prefix(BOM) {
                line = rest;
            }
        }
        if std::str::from_utf8(line).is_err() {
            return Err(Error::Framing(Framing::Utf8));
        }
        let flow = if line.is_empty() {
            if self.has_data {
                self.data.pop();
                let flow = handler(&self.data).map_err(Error::Handler)?;
                self.data.clear();
                self.has_data = false;
                flow
            } else {
                Flow::Continue
            }
        } else if line[0] == b':' {
            Flow::Continue
        } else {
            let (field, value) = match line.iter().position(|b| *b == b':') {
                Some(colon) => {
                    let value = &line[colon + 1..];
                    (&line[..colon], value.strip_prefix(b" ").unwrap_or(value))
                }
                None => (line, &line[line.len()..]),
            };
            if field == b"data" {
                if value.len() >= self.limits.event_bytes - self.data.len() {
                    return Err(Error::Framing(Framing::Event));
                }
                self.data.extend_from_slice(value);
                self.data.push(b'\n');
                self.has_data = true;
            }
            Flow::Continue
        };
        self.line.clear();
        Ok(flow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Historical quality limits: preserve the original framing regression cases.
    const SSE_LINE_CAP: usize = 256 * 1024;
    const SSE_EVENT_CAP: usize = 256 * 1024;

    fn events(chunks: &[&[u8]]) -> Result<(Vec<Vec<u8>>, Option<usize>), Framing> {
        let mut parser = Parser::new(SseLimits {
            line_bytes: SSE_LINE_CAP,
            event_bytes: SSE_EVENT_CAP,
        })
        .unwrap();
        let mut seen = Vec::new();
        for chunk in chunks {
            let stopped = parser
                .feed(chunk, |data| {
                    seen.push(data.to_vec());
                    Ok::<_, ()>(if data == b"[DONE]" {
                        Flow::Stop
                    } else {
                        Flow::Continue
                    })
                })
                .map_err(|e| match e {
                    Error::Framing(f) => f,
                    Error::Handler(()) => unreachable!(),
                })?;
            if stopped.is_some() {
                return Ok((seen, stopped));
            }
        }
        Ok((seen, None))
    }

    #[test]
    fn line_endings_bom_comments_and_splits() {
        let whole = b"\xEF\xBB\xBFdata: a\r\n\r\n: comment\r\ndata:b\ndata: c\r\n\rignored\nfield\ndata\n\n\ndata: [DONE]\n\nsurplus";
        let expected: Vec<Vec<u8>> = vec![
            b"a".to_vec(),
            b"b\nc".to_vec(),
            Vec::new(),
            b"[DONE]".to_vec(),
        ];
        let (seen, stop) = events(&[whole]).unwrap();
        assert_eq!(seen, expected);
        assert_eq!(stop, Some(whole.len() - b"surplus".len()));
        for split in 1..whole.len() {
            let (seen, stop) = events(&[&whole[..split], &whole[split..]]).unwrap();
            assert_eq!(seen, expected, "split at {split}");
            assert!(stop.is_some());
        }
        let bytes: Vec<&[u8]> = whole.chunks(1).collect();
        assert_eq!(events(&bytes).unwrap().0, expected);
    }

    #[test]
    fn no_implicit_event_at_eof_and_strict_utf8() {
        let (seen, stop) = events(&[b"data: partial\n"]).unwrap();
        assert!(seen.is_empty() && stop.is_none());
        let (seen, _) = events(&[b"data: x\n\r"]).unwrap();
        assert_eq!(seen, vec![b"x".to_vec()]);
        let (seen, _) = events(&[b"data: x\r", b"\ndata: y\n\n"]).unwrap();
        assert_eq!(seen, vec![b"x\ny".to_vec()]);
        assert_eq!(
            events(&[b"data: \xC3", b"\xA9\n\n"]).unwrap().0,
            vec!["é".as_bytes().to_vec()]
        );
        assert_eq!(events(&[b"data: \xff\n\n"]), Err(Framing::Utf8));
        assert_eq!(events(&[b": \xff\n"]), Err(Framing::Utf8));
        let long = vec![b'x'; SSE_LINE_CAP + 1];
        assert_eq!(events(&[&long]), Err(Framing::Line));
        let mut many = Vec::new();
        for _ in 0..3 {
            many.extend_from_slice(b"data: ");
            many.extend_from_slice(&vec![b'y'; SSE_LINE_CAP - 16]);
            many.push(b'\n');
        }
        assert_eq!(events(&[&many]), Err(Framing::Event));
    }
    #[test]
    fn explicit_limits_reject_invalid_ranges() {
        assert!(matches!(
            Parser::new(SseLimits {
                line_bytes: 0,
                event_bytes: 8
            }),
            Err(LimitError::Line)
        ));
        assert!(matches!(
            Parser::new(SseLimits {
                line_bytes: 8,
                event_bytes: 0
            }),
            Err(LimitError::Event)
        ));
        assert!(matches!(
            Parser::new(SseLimits {
                line_bytes: usize::MAX,
                event_bytes: 8
            }),
            Err(LimitError::Line)
        ));
        assert!(matches!(
            Parser::new(SseLimits {
                line_bytes: 8,
                event_bytes: usize::MAX
            }),
            Err(LimitError::Event)
        ));
    }

    #[test]
    fn custom_limits_preserve_exact_line_and_event_boundaries() {
        let mut parser = Parser::new(SseLimits {
            line_bytes: 7,
            event_bytes: 3,
        })
        .unwrap();
        let mut seen = Vec::new();
        assert_eq!(
            parser
                .feed(b"data:ab\n\n", |data| {
                    seen.push(data.to_vec());
                    Ok::<_, ()>(Flow::Continue)
                })
                .unwrap(),
            None
        );
        assert_eq!(seen, vec![b"ab".to_vec()]);
        assert_eq!(
            parser.feed(b"data:abc", |_| Ok::<_, ()>(Flow::Continue)),
            Err(Error::Framing(Framing::Line))
        );

        let mut parser = Parser::new(SseLimits {
            line_bytes: 8,
            event_bytes: 3,
        })
        .unwrap();
        assert_eq!(
            parser.feed(b"data:abc\n", |_| Ok::<_, ()>(Flow::Continue)),
            Err(Error::Framing(Framing::Event))
        );
    }

    #[test]
    fn empty_chunks_do_not_break_a_pending_crlf() {
        let mut parser = Parser::new(SseLimits {
            line_bytes: 32,
            event_bytes: 32,
        })
        .unwrap();
        let mut seen = Vec::new();
        for chunk in [b"data:x\r".as_slice(), b"", b"\n"] {
            parser
                .feed(chunk, |data| {
                    seen.push(data.to_vec());
                    Ok::<_, ()>(Flow::Continue)
                })
                .unwrap();
        }
        assert!(seen.is_empty());
        parser
            .feed(b"\n", |data| {
                seen.push(data.to_vec());
                Ok::<_, ()>(Flow::Continue)
            })
            .unwrap();
        assert_eq!(seen, vec![b"x".to_vec()]);
    }

    #[test]
    fn handler_owns_stop_semantics_and_errors() {
        let limits = SseLimits {
            line_bytes: 32,
            event_bytes: 32,
        };
        let mut parser = Parser::new(limits).unwrap();
        let mut seen = Vec::new();
        let stopped = parser
            .feed(b"data:[DONE]\n\ndata:halt\n\nremaining", |data| {
                seen.push(data.to_vec());
                Ok::<_, ()>(if data == b"halt" {
                    Flow::Stop
                } else {
                    Flow::Continue
                })
            })
            .unwrap();
        assert_eq!(seen, vec![b"[DONE]".to_vec(), b"halt".to_vec()]);
        assert_eq!(stopped, Some(b"data:[DONE]\n\ndata:halt\n\n".len()));
        let mut parser = Parser::new(limits).unwrap();
        assert_eq!(
            parser.feed(b"data:x\n\n", |_| Err::<Flow, _>("caller rejected event")),
            Err(Error::Handler("caller rejected event"))
        );
    }
}
