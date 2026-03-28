use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use anstream::{AutoStream, ColorChoice};
use anstyle::{AnsiColor, Style};
use anyhow::Error;

use crate::execution::PreviewChange;

const LABEL_WIDTH: usize = 12;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorMode {
    #[default]
    Auto,
    #[cfg(test)]
    Always,
    Never,
}

impl ColorMode {
    fn choice(self) -> ColorChoice {
        match self {
            Self::Auto => ColorChoice::Auto,
            #[cfg(test)]
            Self::Always => ColorChoice::Always,
            Self::Never => ColorChoice::Never,
        }
    }
}

pub struct Reporter {
    result: ReportStream,
    diagnostic: ReportStream,
}

struct ReportStream {
    writer: Rc<RefCell<Box<dyn Write>>>,
    color_enabled: bool,
}

impl ReportStream {
    fn new(writer: impl Write + 'static, color_enabled: bool) -> Self {
        Self {
            writer: Rc::new(RefCell::new(Box::new(writer))),
            color_enabled,
        }
    }

    fn shared(writer: Rc<RefCell<Box<dyn Write>>>, color_enabled: bool) -> Self {
        Self {
            writer,
            color_enabled,
        }
    }

    fn write_line(&self, line: &str) -> anyhow::Result<()> {
        let mut writer = self.writer.borrow_mut();
        writeln!(writer, "{line}").map_err(Into::into)
    }
}

impl Reporter {
    pub fn stderr() -> Self {
        let stream = AutoStream::new(io::stderr().lock(), ColorMode::Auto.choice());
        let color_enabled = !matches!(stream.current_choice(), ColorChoice::Never);
        Self {
            result: ReportStream::new(stream, color_enabled),
            diagnostic: ReportStream::new(
                AutoStream::new(io::stderr().lock(), ColorMode::Auto.choice()),
                color_enabled,
            ),
        }
    }

    pub fn stdio() -> Self {
        let result = AutoStream::new(io::stdout().lock(), ColorMode::Auto.choice());
        let diagnostic = AutoStream::new(io::stderr().lock(), ColorMode::Auto.choice());
        let result_color_enabled = !matches!(result.current_choice(), ColorChoice::Never);
        let diagnostic_color_enabled = !matches!(diagnostic.current_choice(), ColorChoice::Never);
        Self {
            result: ReportStream::new(result, result_color_enabled),
            diagnostic: ReportStream::new(diagnostic, diagnostic_color_enabled),
        }
    }

    pub fn sink(_mode: ColorMode, writer: impl Write + 'static) -> Self {
        let color_enabled = {
            #[cfg(test)]
            {
                matches!(_mode, ColorMode::Always)
            }
            #[cfg(not(test))]
            {
                false
            }
        };
        let writer = Rc::new(RefCell::new(Box::new(writer) as Box<dyn Write>));
        Self {
            result: ReportStream::shared(Rc::clone(&writer), color_enabled),
            diagnostic: ReportStream::shared(writer, color_enabled),
        }
    }

    #[cfg(test)]
    pub fn sink_split(
        mode: ColorMode,
        result_writer: impl Write + 'static,
        diagnostic_writer: impl Write + 'static,
    ) -> Self {
        let color_enabled = matches!(mode, ColorMode::Always);
        Self {
            result: ReportStream::new(result_writer, color_enabled),
            diagnostic: ReportStream::new(diagnostic_writer, color_enabled),
        }
    }

    pub fn silent() -> Self {
        Self::sink(ColorMode::Never, io::sink())
    }

    pub fn status(&self, label: &str, message: impl std::fmt::Display) -> anyhow::Result<()> {
        let padded = format!("{label:>LABEL_WIDTH$}");
        self.write_diagnostic_line(&format!(
            "{} {message}",
            self.styled_diagnostic(&padded, Self::status_style()),
        ))
    }

    pub fn finish(&self, message: impl std::fmt::Display) -> anyhow::Result<()> {
        let padded = format!("{:>LABEL_WIDTH$}", "Finished");
        self.write_result_line(&format!(
            "{} {message}",
            self.styled_result(&padded, Self::finish_style()),
        ))
    }

    pub fn warning(&self, message: impl std::fmt::Display) -> anyhow::Result<()> {
        self.write_diagnostic_line(&format!(
            "{} {message}",
            self.styled_diagnostic("warning:", Self::warning_style()),
        ))
    }

    pub fn note(&self, message: impl std::fmt::Display) -> anyhow::Result<()> {
        self.write_diagnostic_line(&format!(
            "{} {message}",
            self.styled_diagnostic("note:", Self::note_style()),
        ))
    }

    pub fn line(&self, message: impl std::fmt::Display) -> anyhow::Result<()> {
        self.write_result_line(&message.to_string())
    }

    pub fn preview(&self, change: &PreviewChange) -> anyhow::Result<()> {
        self.note(change.describe())
    }

    pub fn color_enabled(&self) -> bool {
        self.result.color_enabled
    }

    pub fn paint(&self, value: &str, style: Style) -> String {
        self.styled_result(value, style)
    }

    pub fn error(&self, error: &Error) -> anyhow::Result<()> {
        let mut chain = error.chain();
        if let Some(head) = chain.next() {
            self.write_diagnostic_line(&format!(
                "{} {head}",
                self.styled_diagnostic("error:", Self::error_style()),
            ))?;
        }

        let causes = chain.map(|cause| cause.to_string()).collect::<Vec<_>>();
        if !causes.is_empty() {
            self.write_diagnostic_line("Caused by:")?;
            for (index, cause) in causes.iter().enumerate() {
                self.write_diagnostic_line(&format!("  {index}: {cause}"))?;
            }
        }

        Ok(())
    }

    fn write_result_line(&self, line: &str) -> anyhow::Result<()> {
        self.result.write_line(line)
    }

    fn write_diagnostic_line(&self, line: &str) -> anyhow::Result<()> {
        self.diagnostic.write_line(line)
    }

    fn styled_result(&self, value: &str, style: Style) -> String {
        if self.result.color_enabled {
            format!("{style}{value}{style:#}")
        } else {
            value.to_string()
        }
    }

    fn styled_diagnostic(&self, value: &str, style: Style) -> String {
        if self.diagnostic.color_enabled {
            format!("{style}{value}{style:#}")
        } else {
            value.to_string()
        }
    }

    fn status_style() -> Style {
        Style::new().bold().fg_color(Some(AnsiColor::Green.into()))
    }

    fn finish_style() -> Style {
        Self::status_style()
    }

    fn warning_style() -> Style {
        Style::new().bold().fg_color(Some(AnsiColor::Yellow.into()))
    }

    fn note_style() -> Style {
        Style::new().bold().fg_color(Some(AnsiColor::Cyan.into()))
    }

    fn error_style() -> Style {
        Style::new().bold().fg_color(Some(AnsiColor::Red.into()))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn renders_plain_status_output_when_color_is_disabled() {
        let buffer = SharedBuffer::default();
        let reporter = Reporter::sink(ColorMode::Never, buffer.clone());

        reporter.status("Checking", "project graph").unwrap();

        assert_eq!(buffer.contents(), "    Checking project graph\n");
    }

    #[test]
    fn renders_colored_output_when_color_is_forced() {
        let buffer = SharedBuffer::default();
        let reporter = Reporter::sink(ColorMode::Always, buffer.clone());

        reporter.warning("be careful").unwrap();

        let output = buffer.contents();
        assert!(output.contains("\u{1b}["));
        assert!(output.contains("warning:"));
        assert!(output.contains("be careful"));
    }

    #[test]
    fn renders_finish_and_note_output() {
        let buffer = SharedBuffer::default();
        let reporter = Reporter::sink(ColorMode::Never, buffer.clone());

        reporter.note("using shared checkout").unwrap();
        reporter.finish("1 package in 0.01s").unwrap();

        assert_eq!(
            buffer.contents(),
            "note: using shared checkout\n    Finished 1 package in 0.01s\n"
        );
    }

    #[test]
    fn renders_plain_lines_without_prefixes() {
        let buffer = SharedBuffer::default();
        let reporter = Reporter::sink(ColorMode::Never, buffer.clone());

        reporter.line("hello world").unwrap();

        assert_eq!(buffer.contents(), "hello world\n");
    }

    #[test]
    fn renders_error_chains() {
        let buffer = SharedBuffer::default();
        let reporter = Reporter::sink(ColorMode::Never, buffer.clone());
        let error = anyhow::anyhow!("outer").context("middle").context("inner");

        reporter.error(&error).unwrap();

        assert_eq!(
            buffer.contents(),
            "error: inner\nCaused by:\n  0: middle\n  1: outer\n"
        );
    }

    #[test]
    fn routes_results_and_diagnostics_to_different_writers() {
        let result = SharedBuffer::default();
        let diagnostic = SharedBuffer::default();
        let reporter = Reporter::sink_split(ColorMode::Never, result.clone(), diagnostic.clone());

        reporter.line("payload").unwrap();
        reporter.status("Checking", "project graph").unwrap();
        reporter.finish("done").unwrap();

        assert_eq!(result.contents(), "payload\n    Finished done\n");
        assert_eq!(diagnostic.contents(), "    Checking project graph\n");
    }
}
