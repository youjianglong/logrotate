use chrono::Local;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stream {
    Stdin,
    Stdout,
    Stderr,
}

#[derive(Clone, Copy, Debug)]
pub struct PrefixOptions {
    pub timestamp: bool,
    pub stream_label: bool,
}

pub struct LineEncoder {
    stream: Stream,
    options: PrefixOptions,
    pending: Vec<u8>,
}

impl LineEncoder {
    pub fn new(stream: Stream, options: PrefixOptions) -> Self {
        Self {
            stream,
            options,
            pending: Vec::new(),
        }
    }

    pub fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(data);
        let mut records = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line = self.pending.drain(..=end).collect::<Vec<_>>();
            records.push(self.decorate(line));
        }
        records
    }

    pub fn finish(&mut self) -> Option<Vec<u8>> {
        if self.pending.is_empty() {
            None
        } else {
            let line = std::mem::take(&mut self.pending);
            Some(self.decorate(line))
        }
    }

    fn decorate(&self, line: Vec<u8>) -> Vec<u8> {
        let mut prefix = Vec::new();
        if self.options.timestamp {
            prefix.extend_from_slice(
                format!("[{}] ", Local::now().format("%y/%m/%d %H:%M:%S%.3f")).as_bytes(),
            );
        }
        if self.options.stream_label {
            match self.stream {
                Stream::Stdout => prefix.extend_from_slice(b"[O] "),
                Stream::Stderr => prefix.extend_from_slice(b"[E] "),
                Stream::Stdin => {}
            }
        }
        prefix.extend_from_slice(&line);
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::{LineEncoder, PrefixOptions, Stream};

    #[test]
    fn keeps_stream_fragments_separate() {
        let options = PrefixOptions {
            timestamp: false,
            stream_label: true,
        };
        let mut stdout = LineEncoder::new(Stream::Stdout, options);
        let mut stderr = LineEncoder::new(Stream::Stderr, options);

        assert!(stdout.push(b"out").is_empty());
        assert_eq!(stderr.push(b"err\n"), vec![b"[E] err\n".to_vec()]);
        assert_eq!(stdout.push(b"put\n"), vec![b"[O] output\n".to_vec()]);
    }

    #[test]
    fn prefixes_empty_and_unterminated_lines() {
        let options = PrefixOptions {
            timestamp: false,
            stream_label: true,
        };
        let mut encoder = LineEncoder::new(Stream::Stdout, options);
        assert_eq!(encoder.push(b"\nlast"), vec![b"[O] \n".to_vec()]);
        assert_eq!(encoder.finish(), Some(b"[O] last".to_vec()));
    }

    #[test]
    fn leaves_bytes_untouched_without_prefixes() {
        let mut encoder = LineEncoder::new(
            Stream::Stdout,
            PrefixOptions {
                timestamp: false,
                stream_label: false,
            },
        );
        assert_eq!(
            encoder.push(&[0xff, b'\r', b'\n']),
            vec![vec![0xff, b'\r', b'\n']]
        );
        assert_eq!(encoder.finish(), None);
    }
}
